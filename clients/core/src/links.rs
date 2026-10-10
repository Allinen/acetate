//! Links to things in Acetate (`https://<server>/open/<track|album|artist>/<id>`, which the
//! server's page hands to the app as `acetate://open/<kind>/<id>?server=<server>`), and
//! share links' expiry. The clients open them in their own ways; reading them is shared.

use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Track(Uuid),
    Album(Uuid),
    Artist(Uuid),
}

impl Target {
    pub fn kind(self) -> &'static str {
        match self {
            Target::Track(_) => "track",
            Target::Album(_) => "album",
            Target::Artist(_) => "artist",
        }
    }

    pub fn id(self) -> Uuid {
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


/// `days` from now, as the server reads it (RFC 3339, UTC).
pub fn expires_at(days: u32) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    rfc3339(now + days as u64 * 86_400)
}

pub(crate) fn rfc3339(secs: u64) -> String {
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    // Days since 1970-01-01 to a calendar date (Howard Hinnant's civil_from_days).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
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

    #[test]
    fn a_date_is_written_as_the_server_reads_it() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(rfc3339(1_709_164_800 + 86_399), "2024-02-29T23:59:59Z");
    }
}
