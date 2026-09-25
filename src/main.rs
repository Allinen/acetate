mod api;
mod auth;
mod config;
mod db;
mod ingest;
mod ingest_command;
mod model;
mod storage;
mod user_commands;

use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use uuid::Uuid;

use crate::ingest::{IngestOptions, IngestOutcome, ReplaceOptions};
use crate::model::sources::{self, Kind, NewSource, ValidSource};
use crate::storage::BlobStore;

#[derive(Parser)]
#[command(name = "acetate", about = "A self-hosted music server where the database is the library")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the HTTP server.
    Serve,
    /// Import audio files from disk. The files themselves are copied, never modified.
    Import {
        /// Files to import.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Where the files came from (required). Just a label (`--origin vk`) for a custom
        /// source, or a source snapshot as JSON (same format as the upload API's `origin`
        /// field), e.g. '{"platform":"soundcloud","data":{"song_id":555}}'. Applies to every
        /// imported file.
        #[arg(long)]
        origin: String,
        /// Initial status for the new tracks.
        #[arg(long, default_value = "released")]
        status: String,
        /// External source snapshots for the track, as a JSON array (same format as the
        /// upload API's `sources` field). Only allowed when importing a single file.
        #[arg(long)]
        sources: Option<String>,
        /// Create the tracks restricted to these groups (comma-separated names), e.g.
        /// `--restrict-to friends,testers`. An empty value = admins only.
        #[arg(long)]
        restrict_to: Option<String>,
    },
    /// Import a whole folder, reading platform ids from the folder layout to match artists
    /// and albums (see the README, "Automated ingestion"). The metadata comes from the tags.
    Ingest {
        /// The folder to import (all audio files below it).
        dir: PathBuf,
        /// How the folder is laid out: `netease` (`Artist - <id>/…/Album - <id>/… - <song id>.flac`),
        /// `ytdlp` (`Title [<id>].ext`, needs `--platform`) or `tags` (nothing from paths).
        #[arg(long)]
        layout: LayoutArg,
        /// For `--layout ytdlp`: where the files are from.
        #[arg(long)]
        platform: Option<PlatformArg>,
        /// Where the files came from when the path doesn't say (required for `tags`):
        /// a label (`--origin vk`) or a source snapshot as JSON, as for `import --origin`.
        #[arg(long)]
        origin: Option<String>,
        /// A folder of album covers named `<title> - <album id>.jpg` (for `netease`).
        #[arg(long)]
        covers: Option<PathBuf>,
        /// A JSON map of NetEase song ids to `{"album_id", "song_index"}`: the album and
        /// position of songs, also ones outside album folders (album_id -1: no album).
        /// Required with `--layout netease`.
        #[arg(long)]
        netease_albums: Option<PathBuf>,
        /// Initial status for the new tracks.
        #[arg(long, default_value = "released")]
        status: String,
        /// Create the tracks restricted to these groups (comma-separated names).
        #[arg(long)]
        restrict_to: Option<String>,
        /// Work out and print what would happen, without changing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Replace a track's file, e.g. with a higher quality version. The old file is kept.
    ReplaceFile {
        track_id: Uuid,
        path: PathBuf,
        /// Why the file is being replaced, e.g. "flac upgrade".
        #[arg(long, default_value = "")]
        reason: String,
        /// Where the new file came from (required): a label or a source snapshot as JSON
        /// (see `import --origin`).
        #[arg(long)]
        origin: String,
    },
    /// Manage users (e.g. create the first admin: `acetate user create alice --group admin`).
    User {
        #[command(subcommand)]
        command: user_commands::UserCommand,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum LayoutArg {
    Netease,
    Ytdlp,
    Tags,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum PlatformArg {
    Soundcloud,
    Youtube,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("acetate=info,tower_http=info")),
        )
        .init();

    let cli = Cli::parse();
    let config = Config::from_env()?;

    // Say what's happening, and fail fast and clearly if the database isn't there
    // (`connect_owner` has a short timeout).
    let db_host = database_host(&config.database_url);
    tracing::info!("connecting to the database at {db_host}");
    // As the owner: migrations and the CLI act as the system. The server makes its own
    // pool, restricted by row-level security (see `serve`).
    let pool = db::connect_owner(&config.database_url)
        .await
        .with_context(|| {
            format!(
                "can't reach the database at {db_host}. Is Postgres running? \
                 (`docker compose up -d`, check with `docker compose ps`)"
            )
        })?;

    // Applies any migrations in ./migrations that haven't run yet.
    // `migrate!` embeds the SQL files into the binary at compile time.
    sqlx::migrate!()
        .run(&pool)
        .await
        .context("running migrations")?;

    let store = BlobStore::open(&config.data_dir)?;

    match cli.command {
        Command::Serve => serve(config, pool, store).await,
        Command::User { command } => user_commands::run(&pool, command).await,
        Command::Import {
            paths,
            origin,
            status,
            sources,
            restrict_to,
        } => {
            let origin = parse_source(&origin)?;
            let sources = match sources {
                Some(_) if paths.len() != 1 => {
                    anyhow::bail!("--sources can only be used when importing a single file")
                }
                Some(json) => parse_sources(&json)?,
                None => Vec::new(),
            };
            let restrict_to = match restrict_to {
                Some(names) => Some(group_ids(&pool, &names).await?),
                None => None,
            };
            let options = IngestOptions {
                user: None, // the CLI acts as the system
                uploaded_filename: String::new(), // set per file
                origin,
                status,
                sources,
                restrict_to,
                hints: Default::default(),
                local_path: None, // set per file
                given: Default::default(),
            };
            import(pool, store, paths, options).await
        }
        Command::Ingest {
            dir,
            layout,
            platform,
            origin,
            covers,
            netease_albums,
            status,
            restrict_to,
            dry_run,
        } => {
            use crate::ingest::layouts::{ytdlp, Layout};
            let layout = match (layout, platform) {
                (LayoutArg::Netease, None) => Layout::Netease,
                (LayoutArg::Tags, None) => Layout::Tags,
                (LayoutArg::Ytdlp, Some(PlatformArg::Soundcloud)) => Layout::Ytdlp(ytdlp::Platform::Soundcloud),
                (LayoutArg::Ytdlp, Some(PlatformArg::Youtube)) => Layout::Ytdlp(ytdlp::Platform::Youtube),
                (LayoutArg::Ytdlp, None) => anyhow::bail!("--layout ytdlp needs --platform"),
                (_, Some(_)) => anyhow::bail!("--platform is only for --layout ytdlp"),
            };
            let origin = match origin {
                Some(json) => Some(parse_source(&json)?),
                None if layout == Layout::Tags => {
                    anyhow::bail!("--layout tags needs --origin: where did the files come from?")
                }
                None => None,
            };
            let restrict_to = match restrict_to {
                Some(names) => Some(group_ids(&pool, &names).await?),
                None => None,
            };
            // Every NetEase song is in the map, and without it songs outside album folders
            // would get no NetEase album: so the netease layout never runs without it.
            let netease_albums = match (netease_albums, layout) {
                (Some(path), _) => Some(crate::ingest::layouts::netease::AlbumMap::load(&path)?),
                (None, Layout::Netease) => anyhow::bail!(
                    "--layout netease needs --netease-albums <map.json> (the song id -> album map)"
                ),
                (None, _) => None,
            };
            let options = ingest_command::Options {
                layout,
                origin,
                covers,
                netease_albums,
                status,
                restrict_to,
                dry_run,
            };
            ingest_command::run(&pool, &store, &dir, options).await
        }
        Command::ReplaceFile {
            track_id,
            path,
            reason,
            origin,
        } => {
            let origin = parse_source(&origin)?;
            let pending = ingest::receive_path(&store, &path).await?;
            let options = ReplaceOptions {
                user: None, // the CLI acts as the system
                uploaded_filename: file_name(&path),
                origin,
                reason,
            };
            let outcome = ingest::replace_file(&pool, &store, track_id, pending, options).await?;
            println!(
                "track {track_id}: file {} replaced by {}",
                outcome.old_file_id, outcome.new_file_id
            );
            Ok(())
        }
    }
}

/// `owner_pool` is only used for the startup checks. Requests get a pool whose
/// connections run as `db::APP_ROLE`, so row-level security applies to all of them.
async fn serve(config: Config, owner_pool: sqlx::PgPool, store: BlobStore) -> anyhow::Result<()> {
    auth::check_permissions_table(&owner_pool).await?;
    db::check_row_level_security(&owner_pool).await?;
    owner_pool.close().await;
    let pool = db::connect_app(&config.database_url).await?;
    let app = api::router(api::AppState {
        pool,
        store,
        web_dir: config.web_dir.clone(),
        secure_cookies: config.secure_cookies,
        trust_proxy: config.trust_proxy,
        rate_limits: std::sync::Arc::new(api::RateLimits::new()),
    });
    let listener = tokio::net::TcpListener::bind(&config.bind)
        .await
        .with_context(|| format!("binding {}", config.bind))?;
    tracing::info!("listening on http://{}", config.bind);
    // `with_connect_info`: lets handlers see the client's address (for rate limiting).
    let app = app.into_make_service_with_connect_info::<std::net::SocketAddr>();
    axum::serve(listener, app).await?;
    Ok(())
}

/// `options` applies to every file (`uploaded_filename` is filled in per file).
async fn import(
    pool: sqlx::PgPool,
    store: BlobStore,
    paths: Vec<PathBuf>,
    options: IngestOptions,
) -> anyhow::Result<()> {
    let (mut created, mut duplicates, mut skipped, mut failed) = (0, 0, 0, 0);

    for path in paths {
        let options = IngestOptions {
            uploaded_filename: file_name(&path),
            local_path: Some(path.display().to_string()),
            ..options.clone()
        };

        let result = match ingest::receive_path(&store, &path).await {
            Ok(pending) => ingest::ingest(&pool, &store, pending, options).await,
            Err(err) => Err(err),
        };
        match result {
            Ok(IngestOutcome::Created(id)) => {
                created += 1;
                println!("created    {id}  {}", path.display());
            }
            Ok(IngestOutcome::Duplicate(id)) => {
                duplicates += 1;
                // The CLI is the system and sees every track, so the id is always there.
                let id = id.map(|id| id.to_string()).unwrap_or_default();
                println!("duplicate  {id}  {}", path.display());
            }
            Err(err) if ingest::is_refusal(&err) => {
                skipped += 1;
                eprintln!("skipped    {}: {err}", path.display());
            }
            Err(err) => {
                failed += 1;
                eprintln!("error      {}: {err:#}", path.display());
            }
        }
    }

    println!("\n{created} created, {duplicates} duplicates, {skipped} skipped, {failed} failed");
    if skipped + failed > 0 {
        println!("Files that didn't import are listed on the Review page (with their audio).");
    }
    Ok(())
}

/// Comma-separated group names -> their ids. Empty = no groups (admins only).
async fn group_ids(pool: &sqlx::PgPool, names: &str) -> anyhow::Result<Vec<Uuid>> {
    let names: Vec<String> = names
        .split(',')
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .collect();
    let mut tx = db::begin(pool, None).await?;
    match db::groups::ids_by_name(&mut tx, &names).await? {
        Ok(ids) => Ok(ids),
        Err(unknown) => anyhow::bail!("unknown groups: {}", unknown.join(", ")),
    }
}

/// An `--origin`: either a source snapshot as JSON (`{"platform": ..., "data": {...}}`),
/// or just a word or two for where the file came from (`vk`, `a friend's drive`), which is
/// short for a custom source with that label: `{"platform":"custom","data":{"label":"vk"}}`.
fn parse_source(text: &str) -> anyhow::Result<ValidSource> {
    let source: NewSource = if text.trim_start().starts_with('{') {
        serde_json::from_str(text).context("invalid --origin JSON")?
    } else {
        if text.trim().is_empty() {
            anyhow::bail!("--origin is empty: say where the files came from");
        }
        NewSource {
            platform: "custom".to_string(),
            source_id: None,
            captured_at: None,
            data: serde_json::json!({ "label": text.trim() }),
        }
    };
    Ok(sources::validate(Kind::Track, source)?)
}

fn parse_sources(json: &str) -> anyhow::Result<Vec<ValidSource>> {
    let list: Vec<NewSource> = serde_json::from_str(json).context("invalid --sources JSON")?;
    let mut valid = Vec::new();
    for source in list {
        valid.push(sources::validate(Kind::Track, source)?);
    }
    Ok(valid)
}

/// The host:port/database part of a database URL, without the username and password.
fn database_host(url: &str) -> &str {
    url.rsplit_once('@').map_or(url, |(_, host)| host)
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}
