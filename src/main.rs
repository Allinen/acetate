mod api;
mod auth;
mod config;
mod db;
mod ingest;
mod model;
mod storage;
mod user_commands;

use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use uuid::Uuid;

use crate::ingest::{IngestOptions, IngestOutcome, Rejected, ReplaceOptions};
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
        /// Where the files came from (required): one source snapshot as JSON (same format as
        /// the upload API's `origin` field), e.g.
        /// '{"platform":"custom","data":{"label":"discord"}}'. Applies to every imported file.
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
    /// Replace a track's file, e.g. with a higher quality version. The old file is kept.
    ReplaceFile {
        track_id: Uuid,
        path: PathBuf,
        /// Why the file is being replaced, e.g. "flac upgrade".
        #[arg(long, default_value = "")]
        reason: String,
        /// Where the new file came from (required): one source snapshot as JSON
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
            };
            import(pool, store, paths, options).await
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
            Err(err) if err.downcast_ref::<Rejected>().is_some() => {
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

fn parse_source(json: &str) -> anyhow::Result<ValidSource> {
    let source: NewSource = serde_json::from_str(json).context("invalid --origin JSON")?;
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
