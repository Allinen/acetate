//! `acetate user ...`: managing users from the command line. This is how the first admin
//! is created; after that, admins can also manage users through the API.
//!
//! The CLI talks to the database directly, so it acts as the system: no login, no
//! permission checks.

use anyhow::{Context, bail};
use clap::Subcommand;
use sqlx::PgPool;

use crate::auth;
use crate::db;

#[derive(Subcommand)]
pub enum UserCommand {
    /// Create a user. Asks for the password.
    Create {
        username: String,
        /// Group to put the user in; repeat for several. E.g. `--group admin`.
        #[arg(long = "group", default_value = "user")]
        groups: Vec<String>,
        /// Read the password from stdin instead of asking (for scripts).
        #[arg(long)]
        password_stdin: bool,
    },
    /// Set a user's password. Logs them out everywhere.
    SetPassword {
        username: String,
        #[arg(long)]
        password_stdin: bool,
    },
    /// Replace a user's groups.
    SetGroups {
        username: String,
        #[arg(required = true)]
        groups: Vec<String>,
    },
    /// List users and their groups.
    List,
}

pub async fn run(pool: &PgPool, command: UserCommand) -> anyhow::Result<()> {
    match command {
        UserCommand::Create {
            username,
            groups,
            password_stdin,
        } => {
            let username = username.trim();
            if username.is_empty() {
                bail!("username can't be empty");
            }
            let password = read_password(password_stdin)?;
            let hash = auth::hash_password_async(password).await?;

            let mut tx = db::begin(pool, None).await?;
            if db::users::find_user_id(&mut tx, username).await?.is_some() {
                bail!("username `{username}` is taken");
            }
            let id = db::users::create_user(&mut tx, username, &hash).await?;
            set_groups(&mut tx, id, &groups).await?;
            tx.commit().await?;
            println!("created user {username} ({id}), groups: {}", groups.join(", "));
        }
        UserCommand::SetPassword {
            username,
            password_stdin,
        } => {
            let mut tx = db::begin(pool, None).await?;
            let id = find(&mut tx, &username).await?;
            let password = read_password(password_stdin)?;
            let hash = auth::hash_password_async(password).await?;
            db::users::set_password(&mut tx, id, &hash).await?;
            tx.commit().await?;
            db::users::delete_sessions_of(pool, id, None).await?;
            println!("password of {username} changed, all their sessions logged out");
        }
        UserCommand::SetGroups { username, groups } => {
            let mut tx = db::begin(pool, None).await?;
            let id = find(&mut tx, &username).await?;
            set_groups(&mut tx, id, &groups).await?;
            tx.commit().await?;
            println!("groups of {username}: {}", groups.join(", "));
        }
        UserCommand::List => {
            let mut tx = db::begin(pool, None).await?;
            for user in db::users::list_users(&mut tx).await? {
                let password = if user.has_password { "" } else { "  (no password)" };
                println!("{}  {:<20} {}{password}", user.id, user.username, user.groups.join(", "));
            }
        }
    }
    Ok(())
}

async fn find(conn: &mut sqlx::PgConnection, username: &str) -> anyhow::Result<uuid::Uuid> {
    db::users::find_user_id(conn, username)
        .await?
        .with_context(|| format!("no user named `{username}`"))
}

async fn set_groups(
    conn: &mut sqlx::PgConnection,
    user_id: uuid::Uuid,
    groups: &[String],
) -> anyhow::Result<()> {
    // The outer `?` is for database errors; the inner result is "these groups won't do".
    db::users::set_groups(conn, user_id, groups)
        .await?
        .map_err(anyhow::Error::msg)
}

/// Asks for a password twice (without echoing it), or reads one line from stdin.
fn read_password(from_stdin: bool) -> anyhow::Result<String> {
    let password = if from_stdin {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        line.trim_end_matches(['\r', '\n']).to_string()
    } else {
        let password = rpassword::prompt_password("Password: ")?;
        if rpassword::prompt_password("Again: ")? != password {
            bail!("passwords don't match");
        }
        password
    };
    if password.chars().count() < auth::MIN_PASSWORD_LENGTH {
        bail!(
            "password must be at least {} characters",
            auth::MIN_PASSWORD_LENGTH
        );
    }
    Ok(password)
}
