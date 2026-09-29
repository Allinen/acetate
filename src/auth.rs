//! Users, permissions, passwords and session tokens. HTTP-independent: the API's login
//! and permission checks (`api::auth`) and the CLI's user commands both build on this.

use std::str::FromStr;

use anyhow::Context;
use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Everything a user can be allowed to do. Mirrors the `permissions` table; the server
/// refuses to start if the two differ (see `check_permissions_table`).
///
/// `Admin` implies all the others: `CurrentUser::has` always says yes for admins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Admin,
    Browse,
    Upload,
    EditMetadata,
    ViewHistory,
    RestoreRevision,
    EditRelations,
    ManageAccess,
    ManageUsers,
    DownloadOriginal,
    ReplaceFile,
    EditSources,
    Stream,
    Share,
    /// Correcting what's recorded about a file itself (whether it's an original file).
    EditFiles,
}

impl Permission {
    pub const ALL: &[Permission] = &[
        Permission::Admin,
        Permission::Browse,
        Permission::Upload,
        Permission::EditMetadata,
        Permission::ViewHistory,
        Permission::RestoreRevision,
        Permission::EditRelations,
        Permission::ManageAccess,
        Permission::ManageUsers,
        Permission::DownloadOriginal,
        Permission::ReplaceFile,
        Permission::EditSources,
        Permission::Stream,
        Permission::Share,
        Permission::EditFiles,
    ];

    /// The name used in the database and the API.
    pub fn name(self) -> &'static str {
        match self {
            Permission::Admin => "admin",
            Permission::Browse => "browse",
            Permission::Upload => "upload",
            Permission::EditMetadata => "edit_metadata",
            Permission::ViewHistory => "view_history",
            Permission::RestoreRevision => "restore_revision",
            Permission::EditRelations => "edit_relations",
            Permission::ManageAccess => "manage_access",
            Permission::ManageUsers => "manage_users",
            Permission::DownloadOriginal => "download_original",
            Permission::ReplaceFile => "replace_file",
            Permission::EditSources => "edit_sources",
            Permission::Stream => "stream",
            Permission::Share => "share",
            Permission::EditFiles => "edit_files",
        }
    }
}

// Lets you write `"upload".parse::<Permission>()`.
impl FromStr for Permission {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Permission::ALL
            .iter()
            .copied()
            .find(|p| p.name() == name)
            .ok_or_else(|| format!("unknown permission `{name}`"))
    }
}

/// Whoever is making a request: a logged-in user, or a guest (not logged in).
#[derive(Debug, Clone, Serialize)]
pub struct CurrentUser {
    /// None for guests. (`flatten`: in JSON, its fields appear directly on this object.)
    #[serde(flatten)]
    pub login: Option<Login>,
    /// The union of the permissions of all the user's groups. Always empty for guests.
    pub permissions: Vec<Permission>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Login {
    pub id: Uuid,
    pub username: String,
    /// The session the request was made with (so logout knows what to end).
    #[serde(skip)]
    pub session_id: Uuid,
}

impl CurrentUser {
    /// Someone who isn't logged in. Guests have no permissions, and that's deliberately not
    /// configurable: the only thing that opens anything up to a guest is a share link.
    pub fn guest() -> Self {
        CurrentUser {
            login: None,
            permissions: Vec::new(),
        }
    }

    /// The user's id, or None for a guest.
    pub fn id(&self) -> Option<Uuid> {
        self.login.as_ref().map(|login| login.id)
    }

    pub fn has(&self, permission: Permission) -> bool {
        self.permissions.contains(&Permission::Admin) || self.permissions.contains(&permission)
    }

    /// True if the user has at least one of `permissions`.
    pub fn has_any(&self, permissions: &[Permission]) -> bool {
        permissions.iter().any(|p| self.has(*p))
    }
}

/// Makes sure the `permissions` table and the `Permission` enum agree, so a permission
/// can't silently exist in only one of them.
pub async fn check_permissions_table(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    let mut in_db = sqlx::query_scalar!("SELECT permission FROM permissions")
        .fetch_all(pool)
        .await?;
    let mut in_code: Vec<String> = Permission::ALL.iter().map(|p| p.name().to_string()).collect();
    in_db.sort();
    in_code.sort();
    if in_db != in_code {
        anyhow::bail!(
            "the permissions table doesn't match auth::Permission:\n  database: {in_db:?}\n  code:     {in_code:?}"
        );
    }
    Ok(())
}

// ---- Passwords ----

pub const MIN_PASSWORD_LENGTH: usize = 8;

/// Hashes a password with Argon2id and a random salt. The result is a self-describing
/// string (`$argon2id$v=19$m=...`) that includes the salt and parameters.
///
/// Deliberately slow (that's the point of Argon2), so call it via `spawn_blocking` from
/// async code; see `hash_password_async`.
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let hash = Argon2::default()
        .hash_password(password.as_bytes())
        .map_err(|err| anyhow::anyhow!("hashing password: {err}"))?;
    Ok(hash.to_string())
}

/// Checks a password against a hash from `hash_password`.
pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub async fn hash_password_async(password: String) -> anyhow::Result<String> {
    tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .context("password hashing task failed")?
}

/// Verifies on the blocking thread pool. With no stored hash (unknown user, or a user
/// without a password), it checks against a dummy hash anyway, so the response takes as
/// long as for a real user and doesn't reveal which usernames exist.
pub async fn verify_password_async(password: String, hash: Option<String>) -> bool {
    let result = tokio::task::spawn_blocking(move || match hash {
        Some(hash) => verify_password(&password, &hash),
        None => {
            verify_password(&password, dummy_hash());
            false
        }
    })
    .await;
    result.unwrap_or(false)
}

/// A valid hash of a random password, computed once.
fn dummy_hash() -> &'static str {
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HASH.get_or_init(|| hash_password(&new_token()).expect("hashing dummy password"))
}

// ---- Session tokens ----

/// A new random session token: 32 random bytes as hex. This is what the client holds.
pub fn new_token() -> String {
    let bytes: [u8; 32] = rand::random();
    hex::encode(bytes)
}

/// What's stored in the database instead of the token itself. A plain (fast) hash is
/// fine here, unlike for passwords: the token is 256 random bits, so it can't be guessed
/// by trying likely values.
pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let hash = hash_password("correct horse").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password("correct horse", &hash));
        assert!(!verify_password("wrong horse", &hash));
        assert!(!verify_password("correct horse", "not a hash"));
    }

    #[test]
    fn tokens_are_random_hex() {
        let (a, b) = (new_token(), new_token());
        assert_ne!(a, b);
        assert_eq!(a.len(), 64);
        assert_eq!(token_hash(&a).len(), 64);
    }

    #[test]
    fn permission_names_roundtrip() {
        for p in Permission::ALL {
            assert_eq!(p.name().parse::<Permission>(), Ok(*p));
        }
    }

    #[test]
    fn admin_has_everything() {
        let user = |permissions| CurrentUser {
            login: None,
            permissions,
        };
        let admin = user(vec![Permission::Admin]);
        assert!(Permission::ALL.iter().all(|p| admin.has(*p)));

        let guest = user(vec![Permission::Stream]);
        assert!(guest.has(Permission::Stream));
        assert!(!guest.has(Permission::Upload));
        assert!(guest.has_any(&[Permission::Upload, Permission::Stream]));
    }
}
