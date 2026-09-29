use std::path::PathBuf;

use anyhow::Context;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    /// Where blobs are stored: (volume name, folder), the first being the default. From
    /// `ACETATE_VOLUMES`, e.g. `main=/srv/acetate,archive=/mnt/hdd/acetate`. The database
    /// only records the names, so each machine says here where they are. Without it, one
    /// volume `main` in `ACETATE_DATA_DIR` (default `./data`).
    pub volumes: Vec<(String, PathBuf)>,
    /// Where smaller copies of cover images are kept, made on first request (see
    /// `storage::images`). From `ACETATE_CACHE_DIR`; default `cache` in the first volume's
    /// folder. Only ever derived from the originals: it can be emptied at any time.
    pub cache_dir: PathBuf,
    /// Uploads through the API (music, covers, ingest from another machine): at most this
    /// many bytes per request (`ACETATE_MAX_UPLOAD_MB`, default 100: what Cloudflare lets
    /// through), and this many a day, all accounts together (`ACETATE_UPLOAD_DAILY_GB`,
    /// default 20), so a stolen account can't fill the disks. 0: no limit. Imports on the
    /// server itself (`acetate ingest`, `import`) don't go through the API: no limit.
    pub max_upload_bytes: u64,
    pub daily_upload_bytes: u64,
    /// Address the HTTP server listens on, e.g. `127.0.0.1:3000`.
    pub bind: String,
    /// Directory with the web UI's static files.
    pub web_dir: PathBuf,
    /// Only send the session cookie over HTTPS. Turn on when serving over HTTPS
    /// (e.g. behind a reverse proxy); it breaks logging in over plain HTTP.
    pub secure_cookies: bool,
    /// Behind a reverse proxy: take the client's address from `X-Forwarded-For`. Only
    /// turn on if a proxy always sets it, or clients can claim any address they like.
    pub trust_proxy: bool,
}

impl Config {
    /// Reads configuration from environment variables (and `.env`, if present).
    pub fn from_env() -> anyhow::Result<Self> {
        // It's fine if there is no .env file, so the result is ignored.
        let _ = dotenvy::dotenv();

        let data_dir: PathBuf = std::env::var("ACETATE_DATA_DIR")
            .unwrap_or_else(|_| "./data".to_string())
            .into();
        let volumes = match std::env::var("ACETATE_VOLUMES") {
            Ok(text) if !text.trim().is_empty() => parse_volumes(&text)?,
            _ => vec![("main".to_string(), data_dir.clone())],
        };
        let cache_dir = match std::env::var("ACETATE_CACHE_DIR") {
            Ok(dir) if !dir.trim().is_empty() => PathBuf::from(dir),
            _ => volumes[0].1.join("cache"),
        };
        let number = |name: &str, default: u64| -> anyhow::Result<u64> {
            match std::env::var(name) {
                Ok(text) if !text.trim().is_empty() => text.trim().parse().with_context(|| format!("{name}: \"{text}\" isn't a whole number")),
                _ => Ok(default),
            }
        };
        Ok(Config {
            max_upload_bytes: number("ACETATE_MAX_UPLOAD_MB", 100)? * 1_000_000,
            daily_upload_bytes: number("ACETATE_UPLOAD_DAILY_GB", 20)? * 1_000_000_000,
            database_url: std::env::var("DATABASE_URL").context("DATABASE_URL must be set")?,
            volumes,
            cache_dir,
            bind: std::env::var("ACETATE_BIND").unwrap_or_else(|_| "127.0.0.1:3000".to_string()),
            web_dir: std::env::var("ACETATE_WEB_DIR")
                .unwrap_or_else(|_| "./web".to_string())
                .into(),
            secure_cookies: std::env::var("ACETATE_SECURE_COOKIES").is_ok_and(|v| v == "true"),
            trust_proxy: std::env::var("ACETATE_TRUST_PROXY").is_ok_and(|v| v == "true"),
        })
    }
}

/// `main=/srv/acetate,archive=/mnt/hdd/acetate` → [(main, /srv/acetate), (archive, ...)].
/// Names are lowercase letters, digits, `-` and `_`, each used once.
pub fn parse_volumes(text: &str) -> anyhow::Result<Vec<(String, PathBuf)>> {
    let mut volumes: Vec<(String, PathBuf)> = Vec::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (name, path) = part
            .split_once('=')
            .with_context(|| format!("ACETATE_VOLUMES: \"{part}\" should be name=/path"))?;
        let name = name.trim();
        let valid = !name.is_empty()
            && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
        if !valid {
            anyhow::bail!("ACETATE_VOLUMES: \"{name}\" isn't a valid volume name (a-z, 0-9, - and _)");
        }
        if volumes.iter().any(|(n, _)| n == name) {
            anyhow::bail!("ACETATE_VOLUMES: \"{name}\" is given twice");
        }
        volumes.push((name.to_string(), PathBuf::from(path.trim())));
    }
    if volumes.is_empty() {
        anyhow::bail!("ACETATE_VOLUMES is empty");
    }
    Ok(volumes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volumes_from_the_environment_format() {
        let v = parse_volumes("main=/srv/acetate, archive=/mnt/hdd/acetate").unwrap();
        assert_eq!(v, vec![
            ("main".to_string(), PathBuf::from("/srv/acetate")),
            ("archive".to_string(), PathBuf::from("/mnt/hdd/acetate")),
        ]);
        assert!(parse_volumes("main").is_err());
        assert!(parse_volumes("Main=/x").is_err());
        assert!(parse_volumes("a=/x,a=/y").is_err());
    }
}
