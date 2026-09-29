//! Links to things in Acetate, to send to someone who has an account:
//! `https://<server>/open/<track|album|artist>/<id>`. The server's page for it hands it to
//! this app (if the browser is logged in) as `acetate://open/<kind>/<id>?server=<server>`,
//! the address the app is registered for (by `install.sh`; Settings says whether it is).
//!
//! The app runs once: a second start with a link (the system starting it for an
//! `acetate://` address) hands the link to the one running, through a socket in the
//! user's runtime folder, and ends; the running one comes to the front and opens it.
//! (Unix only for now: on Windows each link starts the app anew.)

use std::sync::mpsc;

use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Track(Uuid),
    Album(Uuid),
    Artist(Uuid),
}

impl Target {
    fn kind(self) -> &'static str {
        match self {
            Target::Track(_) => "track",
            Target::Album(_) => "album",
            Target::Artist(_) => "artist",
        }
    }

    fn id(self) -> Uuid {
        match self {
            Target::Track(id) | Target::Album(id) | Target::Artist(id) => id,
        }
    }
}

/// A link the app was opened with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub target: Target,
    /// The server it's on (as the link says; None: it doesn't say).
    pub server: Option<String>,
}

/// The link to share for `target` on `server` (a web address: it works in any browser).
pub fn web(server: &str, target: Target) -> String {
    format!("{}/open/{}/{}", server.trim_end_matches('/'), target.kind(), target.id())
}

/// Reads `acetate://open/<kind>/<id>?server=…` (or a web link, `…/open/<kind>/<id>`).
pub fn parse(link: &str) -> Option<Link> {
    let (path, query) = link.split_once('?').unwrap_or((link, ""));
    let rest = match path.strip_prefix("acetate://") {
        Some(rest) => rest.strip_prefix("open/")?,
        None => &path[path.find("/open/")? + "/open/".len()..],
    };
    let (kind, id) = rest.trim_end_matches('/').split_once('/')?;
    let id: Uuid = id.parse().ok()?;
    let target = match kind {
        "track" => Target::Track(id),
        "album" => Target::Album(id),
        "artist" => Target::Artist(id),
        _ => return None,
    };
    let server = query.split('&').find_map(|p| p.strip_prefix("server=")).map(decode).or_else(|| {
        // A web link says its server itself.
        (!path.starts_with("acetate://")).then(|| path[..path.find("/open/").unwrap_or(0)].to_string())
    });
    Some(Link { target, server })
}

/// `%XX` escapes undone (a server address in a query).
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = text.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether two server addresses are the same server (a trailing slash, the case of the
/// host don't matter).
pub fn same_server(a: &str, b: &str) -> bool {
    a.trim_end_matches('/').eq_ignore_ascii_case(b.trim_end_matches('/'))
}

#[cfg(unix)]
fn socket_path() -> std::path::PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let user = std::env::var("USER").unwrap_or_default();
    dir.join(format!("acetate-desktop-{user}.sock"))
}

/// Hands `link` to the app if it's running already. True if it took it (then this start
/// has nothing more to do).
pub fn hand_over(link: &str) -> bool {
    #[cfg(unix)]
    {
        hand_over_to(&socket_path(), link)
    }
    #[cfg(not(unix))]
    {
        let _ = link;
        false
    }
}

#[cfg(unix)]
fn hand_over_to(path: &std::path::Path, link: &str) -> bool {
    use std::io::Write;
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(mut stream) => writeln!(stream, "{link}").is_ok(),
        Err(_) => false,
    }
}

/// Listens for links handed over by later starts: each arrives on the channel, and the
/// window is woken and brought to the front. None if another app is listening already
/// (or it can't listen: then later starts open their own window).
pub fn listen(ctx: eframe::egui::Context) -> Option<mpsc::Receiver<String>> {
    #[cfg(unix)]
    {
        listen_at(&socket_path(), ctx)
    }
    #[cfg(not(unix))]
    {
        let _ = ctx;
        None
    }
}

#[cfg(unix)]
fn listen_at(path: &std::path::Path, ctx: eframe::egui::Context) -> Option<mpsc::Receiver<String>> {
    use std::io::BufRead;
    use std::os::unix::net::{UnixListener, UnixStream};
    let path = path.to_path_buf();
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        // There already: another app is listening, or one ended without cleaning up.
        Err(_) if UnixStream::connect(&path).is_ok() => return None,
        Err(_) => {
            let _ = std::fs::remove_file(&path);
            UnixListener::bind(&path).ok()?
        }
    };
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            if std::io::BufReader::new(stream).read_line(&mut line).is_ok() && !line.trim().is_empty() {
                if send.send(line.trim().to_string()).is_err() {
                    break;
                }
                ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Focus);
                ctx.request_repaint();
            }
        }
    });
    Some(receive)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_read_both_ways() {
        let id = Uuid::from_u128(7);
        let app = format!("acetate://open/album/{id}?server=https%3A%2F%2Fmusic.example.com");
        assert_eq!(parse(&app), Some(Link { target: Target::Album(id), server: Some("https://music.example.com".into()) }));
        let shared = web("https://music.example.com/", Target::Track(id));
        assert_eq!(shared, format!("https://music.example.com/open/track/{id}"));
        assert_eq!(parse(&shared), Some(Link { target: Target::Track(id), server: Some("https://music.example.com".into()) }));
        assert_eq!(parse("acetate://open/playlist/x"), None);
        assert_eq!(parse(&format!("acetate://open/artist/{id}")).unwrap().server, None);
        assert!(same_server("https://Music.example.com/", "https://music.example.com"));
    }

    /// A second start hands its link to the first, which gets it; a third can't listen
    /// too; a socket left behind by an app that ended is taken over.
    #[cfg(unix)]
    #[test]
    fn a_link_is_handed_to_the_app_running() {
        let dir = std::env::temp_dir().join(format!("acetate-links-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app.sock");
        assert!(!hand_over_to(&path, "acetate://open/track/x"), "nothing running yet");

        let ctx = eframe::egui::Context::default();
        let links = listen_at(&path, ctx.clone()).expect("the first listens");
        assert!(listen_at(&path, ctx.clone()).is_none(), "a second doesn't");
        assert!(hand_over_to(&path, "acetate://open/album/y"));
        let got = links.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert_eq!(got, "acetate://open/album/y");

        // Left behind (the listener gone, the file still there): taken over.
        let stale = dir.join("stale.sock");
        drop(std::os::unix::net::UnixListener::bind(&stale).unwrap());
        assert!(listen_at(&stale, ctx).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
