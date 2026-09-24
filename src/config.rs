use std::path::PathBuf;

use anyhow::Context;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    /// Root of all stored data: `blobs/` and `tmp/` live in here.
    pub data_dir: PathBuf,
    /// Address the HTTP server listens on, e.g. `127.0.0.1:3000`.
    pub bind: String,
    /// Directory with the web UI's static files.
    pub web_dir: PathBuf,
}

impl Config {
    /// Reads configuration from environment variables (and `.env`, if present).
    pub fn from_env() -> anyhow::Result<Self> {
        // It's fine if there is no .env file, so the result is ignored.
        let _ = dotenvy::dotenv();

        Ok(Config {
            database_url: std::env::var("DATABASE_URL").context("DATABASE_URL must be set")?,
            data_dir: std::env::var("ACETATE_DATA_DIR")
                .unwrap_or_else(|_| "./data".to_string())
                .into(),
            bind: std::env::var("ACETATE_BIND").unwrap_or_else(|_| "127.0.0.1:3000".to_string()),
            web_dir: std::env::var("ACETATE_WEB_DIR")
                .unwrap_or_else(|_| "./web".to_string())
                .into(),
        })
    }
}
