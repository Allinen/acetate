//! Who's logged in and what they may do (`GET /api/auth/me`), and the server's fixed
//! lists (`GET /api/vocab`: statuses, credit roles, album kinds, relation kinds; and
//! `GET /api/source-shapes`), which edit forms choose from. Fetched after logging in (and at start), in the background.
//!
//! `me` is kept on disk too (`me-<key>.json`, per server and user), so an app started
//! offline still knows what its user may do; the server's answer replaces it.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::api::Api;

/// The API version this client speaks: the number in the repository's `API_VERSION` file,
/// the same one the server is built with.
pub fn api_version() -> u32 {
    include_str!("../../../API_VERSION").trim().parse().expect("API_VERSION holds a number")
}

/// What the server says it is (`GET /api/version`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ServerVersion {
    /// Its release, for people ("0.1.0").
    pub version: String,
    pub api: u32,
}

/// Whether this app and the server can work together.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Compat {
    Same,
    /// The server is newer than the app: update the app.
    AppBehind,
    /// The app is newer than the server: update the server.
    ServerBehind,
}

/// How the server's release compares to this app's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Release {
    Same,
    /// The server's is newer: the app can be brought up to it.
    ServerNewer,
    /// The server's is older: the app stays (an update isn't always usable backwards: an
    /// older server may not read what a newer app keeps), and the server needs updating.
    ServerOlder,
    /// Not a version number we can order.
    Unknown,
}

/// `1.2.3` (or `1.2.3-rc1`, `1.2.3+x`) as numbers.
fn numbers(version: &str) -> Option<[u64; 3]> {
    let mut parts = version.split(['-', '+']).next()?.split('.');
    let mut out = [0; 3];
    for n in &mut out {
        *n = parts.next()?.parse().ok()?;
    }
    parts.next().is_none().then_some(out)
}

impl ServerVersion {
    pub fn release(&self) -> Release {
        if self.matches_app() {
            return Release::Same;
        }
        match (numbers(&self.version), numbers(env!("CARGO_PKG_VERSION"))) {
            (Some(server), Some(ours)) => match server.cmp(&ours) {
                std::cmp::Ordering::Greater => Release::ServerNewer,
                std::cmp::Ordering::Less => Release::ServerOlder,
                std::cmp::Ordering::Equal => Release::Unknown,
            },
            _ => Release::Unknown,
        }
    }

    /// The desktop app of this server's release, for `os` ("linux" or "windows"), to
    /// download straight away.
    pub fn desktop_url(&self, os: &str) -> String {
        let ext = if os == "windows" { "zip" } else { "tar.gz" };
        format!("{RELEASES}/download/v{0}/Acetate-v{0}-{os}-x86_64.{ext}", self.version)
    }

    pub fn compat(&self) -> Compat {
        match self.api.cmp(&api_version()) {
            std::cmp::Ordering::Equal => Compat::Same,
            std::cmp::Ordering::Greater => Compat::AppBehind,
            std::cmp::Ordering::Less => Compat::ServerBehind,
        }
    }

    /// Whether the app is the server's release: a release is the same version everywhere
    /// (server, core, desktop, Android), so any other one is a different build.
    pub fn matches_app(&self) -> bool {
        self.version == env!("CARGO_PKG_VERSION")
    }

    /// The page of the app that goes with this server (where the downloads are).
    pub fn release_url(&self) -> String {
        format!("{RELEASES}/tag/v{}", self.version)
    }

    /// The Android app that goes with this server, to download straight away.
    pub fn apk_url(&self) -> String {
        format!("{RELEASES}/download/v{0}/Acetate-v{0}-android.apk", self.version)
    }

    /// What to tell the person when the versions differ (None: they match).
    pub fn warning(&self) -> Option<String> {
        let ours = env!("CARGO_PKG_VERSION");
        match self.compat() {
            Compat::AppBehind => Some(format!("This server (Acetate {}) is newer than this app ({ours}): update the app, or some things won't work.", self.version)),
            Compat::ServerBehind => Some(format!("This app ({ours}) is newer than this server (Acetate {}): update the server, or some things won't work.", self.version)),
            Compat::Same if self.matches_app() => None,
            Compat::Same if self.release() == Release::ServerOlder => Some(format!("This app ({ours}) is newer than this server (Acetate {}): update the server, or some things won't work.", self.version)),
            Compat::Same => Some(format!("This app ({ours}) isn't the version of this server (Acetate {0}): get the app for {0} from {1}", self.version, self.release_url())),
        }
    }
}

const RELEASES: &str = "https://github.com/Allinen/acetate/releases";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Me {
    pub permissions: HashSet<String>,
}

impl Me {
    /// Whether they may (`admin` may everything).
    pub fn can(&self, permission: &str) -> bool {
        self.permissions.contains("admin") || self.permissions.contains(permission)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Vocab {
    pub statuses: Vec<String>,
    pub credit_roles: Vec<String>,
    /// For album forms (not yet).
    #[allow(dead_code)]
    pub album_kinds: Vec<String>,
    pub relation_kinds: Vec<RelationKind>,
    /// The kinds of review flag, in the order to show them.
    pub review_kinds: Vec<ReviewKind>,
    /// What each platform's sources hold (from the extensions, `GET /api/extensions`, fetched with it).
    #[serde(skip)]
    pub source_shapes: Vec<SourceShape>,
}

/// The server's extensions (kept for reading links), and the source shapes they have.
async fn fetch_shapes(api: &Api) -> Vec<SourceShape> {
    match api.send_json::<crate::extensions::Registry>(reqwest::Method::GET, "/api/extensions", None).await {
        Ok(registry) => {
            let shapes = shapes_of(&registry);
            crate::extensions::set(registry);
            shapes
        }
        Err(_) => shapes_of(&crate::extensions::current()),
    }
}

/// The source shapes of a set of extensions.
pub fn shapes_of(registry: &crate::extensions::Registry) -> Vec<SourceShape> {
    registry
        .shapes()
        .map(|(platform, shape)| SourceShape {
            platform: platform.to_string(),
            kind: shape.kind.clone(),
            id_field: shape.id.clone(),
            fields: shape
                .fields
                .iter()
                .map(|f| ShapeField {
                    name: f.name.clone(),
                    kind: match f.kind {
                        acetate_extensions::FieldType::Int => "int",
                        acetate_extensions::FieldType::Str => "str",
                        acetate_extensions::FieldType::Strs => "strs",
                    }
                    .to_string(),
                    required: f.required,
                })
                .collect(),
        })
        .collect()
}

/// A platform's sources of one kind (track, album, artist) and their fields.
#[derive(Debug, Clone, Deserialize)]
pub struct SourceShape {
    pub platform: String,
    pub kind: String,
    /// The field with the platform's id: sources with the same id are one source.
    pub id_field: Option<String>,
    pub fields: Vec<ShapeField>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShapeField {
    pub name: String,
    /// "int", "str" or "strs" (a list of strings).
    #[serde(rename = "type")]
    pub kind: String,
    pub required: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReviewKind {
    pub kind: String,
    /// For people ("No cover").
    pub name: String,
    /// Raised and settled by the server itself, from what the track has ("no cover").
    pub automatic: bool,
    /// Made by people, from the app (raised and settled by hand, like "Fix later").
    #[serde(default)]
    pub custom: bool,
}

impl ReviewKind {
    /// Whether people flag tracks with it (in the editor: `flag:…`).
    pub fn by_hand(&self) -> bool {
        self.custom || self.kind == "fix_later"
    }
}

impl Vocab {
    /// A kind of flag as people read it (a kind not in the list: as the server names it).
    pub fn review_kind_name<'a>(&'a self, kind: &'a str) -> &'a str {
        self.review_kinds.iter().find(|k| k.kind == kind).map_or(kind, |k| k.name.as_str())
    }

    pub fn is_automatic(&self, kind: &str) -> bool {
        self.review_kinds.iter().any(|k| k.kind == kind && k.automatic)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RelationKind {
    pub kind: String,
    /// "%A is a remix of %B": %A is this track, %B the other.
    pub template: String,
}

/// What the server answered: `me`, `vocab` and its version (each None if it didn't).
type Arrived = (Option<Me>, Option<Vocab>, Option<ServerVersion>);

/// `me` and `vocab`, as they arrive.
#[derive(Default)]
pub struct Account {
    pub me: Option<Me>,
    pub vocab: Option<Arc<Vocab>>,
    /// The server's version, as of the last time it answered.
    pub server: Option<ServerVersion>,
    /// Where `me` is kept.
    file: Option<PathBuf>,
    arrived: Arc<Mutex<Option<Arrived>>>,
}

impl Account {
    /// Whose account this is: `me` as kept in `file` (None: logged out), until the server
    /// says.
    pub fn open(file: Option<PathBuf>) -> Account {
        let me = file.as_ref().and_then(|f| std::fs::read(f).ok()).and_then(|bytes| serde_json::from_slice(&bytes).ok());
        Account { me, file, ..Account::default() }
    }

    /// Asks the server (again: at start, after a login, when it's back online). What's
    /// known stays until the answer; no answer keeps it.
    pub fn fetch(&mut self, api: &Api, runtime: &tokio::runtime::Handle, waker: &crate::Waker) {
        let (api, arrived, waker, file) = (api.clone(), self.arrived.clone(), waker.clone(), self.file.clone());
        runtime.spawn(async move {
            let me = api.send_json::<Me>(reqwest::Method::GET, "/api/auth/me", None).await.ok();
            if let (Some(me), Some(file)) = (&me, &file)
                && let Err(err) = std::fs::write(file, serde_json::to_vec(me).unwrap_or_default())
            {
                eprintln!("couldn't keep what the account may do: {err}");
            }
            let mut vocab = api.send_json::<Vocab>(reqwest::Method::GET, "/api/vocab", None).await.ok();
            if let Some(vocab) = vocab.as_mut() {
                vocab.source_shapes = fetch_shapes(&api).await;
            }
            let server = api.send_json::<ServerVersion>(reqwest::Method::GET, "/api/version", None).await.ok();
            *arrived.lock().unwrap() = Some((me, vocab, server));
            waker.changed();
        });
    }

    /// Takes in what arrived. Call once per frame.
    pub fn poll(&mut self) {
        if let Some((me, vocab, server)) = self.arrived.lock().unwrap().take() {
            if me.is_some() {
                self.me = me;
            }
            if server.is_some() {
                self.server = server;
            }
            if let Some(vocab) = vocab {
                self.vocab = Some(Arc::new(vocab));
            }
        }
    }

    /// Asks for the vocabulary again (e.g. a kind of flag was made), keeping who's logged
    /// in.
    pub fn refresh_vocab(&mut self, api: &Api, runtime: &tokio::runtime::Handle, waker: &crate::Waker) {
        let (api, arrived, waker) = (api.clone(), self.arrived.clone(), waker.clone());
        runtime.spawn(async move {
            let mut vocab = api.send_json::<Vocab>(reqwest::Method::GET, "/api/vocab", None).await.ok();
            if let Some(vocab) = vocab.as_mut() {
                vocab.source_shapes = fetch_shapes(&api).await;
            }
            if vocab.is_some() {
                *arrived.lock().unwrap() = Some((None, vocab, None));
                waker.changed();
            }
        });
    }

    pub fn can(&self, permission: &str) -> bool {
        self.me.as_ref().is_some_and(|me| me.can(permission))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_account_may_do_is_kept_for_the_next_start() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("me-x.json");
        std::fs::write(&file, br#"{"permissions":["admin"]}"#).unwrap();
        let account = Account::open(Some(file.clone()));
        assert!(account.can("edit_metadata"));
        // Logged out: nothing.
        assert!(!Account::open(None).can("edit_metadata"));
        // An answer that didn't come keeps what's known.
        let mut account = account;
        *account.arrived.lock().unwrap() = Some((None, None, None));
        account.poll();
        assert!(account.can("admin"));
    }

    #[test]
    fn a_server_of_another_release_points_to_its_app() {
        let ours = env!("CARGO_PKG_VERSION");
        let server = |version: &str| ServerVersion { version: version.into(), api: api_version() };
        assert!(server(ours).warning().is_none());
        let other = server("9.9.9");
        assert!(other.warning().unwrap().contains("releases/tag/v9.9.9"));
        assert_eq!(other.apk_url(), "https://github.com/Allinen/acetate/releases/download/v9.9.9/Acetate-v9.9.9-android.apk");
    }

    #[test]
    fn releases_are_ordered_by_their_numbers() {
        let server = |version: &str| ServerVersion { version: version.into(), api: api_version() };
        assert_eq!(server(env!("CARGO_PKG_VERSION")).release(), Release::Same);
        assert_eq!(server("999.0.0").release(), Release::ServerNewer);
        assert_eq!(server("0.0.1").release(), Release::ServerOlder);
        assert_eq!(server("0.0.1-rc1").release(), Release::ServerOlder);
        assert_eq!(server("nightly").release(), Release::Unknown);
        assert_eq!(numbers("0.10.2"), Some([0, 10, 2]));
        assert_eq!(numbers("1.2"), None);
        assert!(server("0.0.1").warning().unwrap().contains("update the server"));
        assert_eq!(server("9.9.9").desktop_url("windows"), "https://github.com/Allinen/acetate/releases/download/v9.9.9/Acetate-v9.9.9-windows-x86_64.zip");
        assert_eq!(server("9.9.9").desktop_url("linux"), "https://github.com/Allinen/acetate/releases/download/v9.9.9/Acetate-v9.9.9-linux-x86_64.tar.gz");
    }

    #[test]
    fn a_server_of_another_api_version_is_warned_about() {
        let server = |api| ServerVersion { version: env!("CARGO_PKG_VERSION").into(), api };
        assert_eq!(server(api_version()).compat(), Compat::Same);
        assert!(server(api_version()).warning().is_none());
        assert_eq!(server(api_version() + 1).compat(), Compat::AppBehind);
        assert!(server(api_version() + 1).warning().unwrap().contains("update the app"));
        assert_eq!(server(api_version() - 1).compat(), Compat::ServerBehind);
        assert!(server(api_version() - 1).warning().unwrap().contains("update the server"));
    }
}
