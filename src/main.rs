mod api;
mod auth;
mod config;
mod db;
mod duplicate_albums_command;
mod ingest;
mod ingest_client;
mod ingest_command;
mod merge_duplicates_command;
mod model;
mod navidrome;
mod storage;
mod user_commands;
mod verify_command;

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
        /// Which storage volume the audio goes to (a name from ACETATE_VOLUMES; default:
        /// the first).
        #[arg(long)]
        volume: Option<String>,
    },
    /// Import a whole folder, reading platform ids from the folder layout to match artists
    /// and albums (see the README, "Automated ingestion"). The metadata comes from the tags.
    Ingest(IngestArgs),
    /// Check, without changing anything, that files are safely stored in Acetate: every
    /// file below <dir> (e.g. an old library, before deleting it by hand), or with
    /// `--blobs`, every stored blob. Never writes, moves or deletes anything in <dir>.
    Verify {
        dir: Option<PathBuf>,
        /// Check the blob store instead: every blob in use is there with the right content,
        /// and which blobs nothing uses anymore.
        #[arg(long)]
        blobs: bool,
        /// Write the paths of the files that are safely stored to this file.
        #[arg(long)]
        list_ok: Option<PathBuf>,
        /// Write the paths of the files that aren't to this file.
        #[arg(long)]
        list_missing: Option<PathBuf>,
    },
    /// Merge the "possible duplicate" pairs from review flags that have exactly the same
    /// title and about the same length. The older track is kept (its metadata and covers),
    /// with the bigger of the two files. Each merge can be undone on the track's page.
    MergeDuplicates {
        /// How far apart the lengths may be, in milliseconds.
        #[arg(long, default_value_t = 1000)]
        max_length_difference_ms: i64,
        /// Also merge titles that differ only in case ("Intro" and "INTRO"). The kept
        /// track's spelling stays.
        #[arg(long)]
        ignore_case: bool,
        /// Only list what would be merged.
        #[arg(long)]
        dry_run: bool,
    },
    /// List albums that look like the same album twice: about the same number of tracks
    /// and total length, a similar title and an album artist in common (e.g. split by
    /// ingest because the album artists or the date differ). Read-only unless --merge.
    DuplicateAlbums {
        /// How many tracks the two may differ by.
        #[arg(long, default_value_t = 0)]
        max_track_difference: i64,
        /// How far apart the total lengths may be, in milliseconds.
        #[arg(long, default_value_t = 2000)]
        max_length_difference_ms: i64,
        /// How alike the titles must be, 0 to 1 (trigram similarity; 1 = the same
        /// ignoring case).
        #[arg(long, default_value_t = 0.6)]
        min_title_similarity: f32,
        /// Also pair albums without an album artist in common (by default one is needed:
        /// singles with a common title like "Intro" by different artists look alike).
        #[arg(long)]
        any_artists: bool,
        /// Merge each pair: the newer album into the older one (tracks, covers, sources,
        /// shares), which keeps its own title, date and credits.
        #[arg(long)]
        merge: bool,
    },
    /// One-off, for merges made before 2026-09-26: takes a merged track's covers back out
    /// of the trash where the kept track had none; moves covers album merges left on the
    /// merged album; and gives an album a copy of the cover a merged album without covers
    /// of its own (e.g. a single) showed from its tracklist. Album fixes are recorded in
    /// each merge, so an undo takes them back too. For merges from importing a stored file
    /// again (before 2026-09-27): an album that import brought (e.g. the single) without
    /// covers gets copies of the covers it came with.
    FixMergedCovers {
        /// Only list what would be moved.
        #[arg(long)]
        dry_run: bool,
    },
    /// Move users and their plays, stars and ratings over from a Navidrome server. Reads
    /// Navidrome's database and music files, never changes them. Matches tracks by their
    /// bytes, so the music files must still be where Navidrome sees them (see --path-map).
    ImportNavidrome {
        /// Navidrome's database file (navidrome.db).
        #[arg(long)]
        db: PathBuf,
        /// Where Navidrome's paths are on this machine, e.g. `--path-map /music=/home/me/Music`
        /// (Docker mounts). Can be given several times; the longest match wins.
        #[arg(long = "path-map")]
        path_maps: Vec<String>,
        /// Only this Navidrome library (its name or id), optionally with where its folder
        /// is here: `--library "Music Library=/home/me/Music"`. Can be given several times;
        /// without it, all libraries. For libraries whose files are on another machine.
        #[arg(long = "library")]
        libraries: Vec<String>,
        /// Navidrome's `PasswordEncryptionKey`, if it was set.
        #[arg(long, default_value = navidrome::DEFAULT_PASSWORD_KEY)]
        password_key: String,
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
        /// Which storage volume the audio goes to (a name from ACETATE_VOLUMES; default:
        /// the first).
        #[arg(long)]
        volume: Option<String>,
    },
    /// Manage users (e.g. create the first admin: `acetate user create alice --group admin`).
    User {
        #[command(subcommand)]
        command: user_commands::UserCommand,
    },
}

/// `acetate ingest`'s options (a struct of its own, so `--server` can run it without a
/// database here).
#[derive(clap::Args)]
struct IngestArgs {
    /// The folder to import (all audio files below it), or a single file.
    dir: PathBuf,
    /// How the folder is laid out: `netease`, `qq` or `kuwo` (`Artist - <id>/…/Album - <id>/… - <song id>.flac`),
    /// `soundcloud` (`<user id>/{FLACs|streams}/…/… - <song id> - <song uid>.wav`),
/// `instrumentals` (`<producer>/<site>@<account>/…`; `--id-map`: the Traktrain lookup),
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
    /// A JSON map of song ids to `{"album_id", "song_index"}` (QQ also: `song_mid`,
    /// `album_mid`, `song_artists`): the album and position of songs, also ones outside
    /// album folders (album_id -1 or 0: no album). Required with `--layout netease`, `qq`
    /// and `kuwo`. (`--netease-albums` is its old name.) For `--layout soundcloud`, the
    /// SoundCloud lookup (songs, albums and uploaders, see the README); also required.
    #[arg(long, alias = "netease-albums")]
    id_map: Option<PathBuf>,
    /// Initial status for the new tracks.
    #[arg(long, default_value = "released")]
    status: String,
    /// Create the tracks restricted to these groups (comma-separated names).
    #[arg(long)]
    restrict_to: Option<String>,
    /// Which storage volume the audio goes to (a name from ACETATE_VOLUMES; default:
    /// the first).
    #[arg(long)]
    volume: Option<String>,
    /// Stop at the first file that can't be reflinked instead of copying it (a
    /// reflink takes no extra space; a copy takes the file's full size).
    #[arg(long)]
    reflink_only: bool,
    /// For `netease`/`qq`/`kuwo`: a file without a song id in its name is looked up in
    /// the id map by its tags' title and artist (only an exact, single match). For
    /// `kuwo`, one that isn't found still comes in, with Kuwo as its origin but no song id.
    #[arg(long)]
    tag_fallback: bool,
    /// When the ids from the path or id map are for a name the tags don't have, and
    /// exactly one tag artist has no ids, they go to that artist: matched by the
    /// platform's name, with the tags' spelling as an alias (e.g. a "jaywren" folder
    /// for a file tagged "mothfecal").
    #[arg(long)]
    lax_artist_name: bool,
    /// Don't credit the COMPOSER tag's names as producers (e.g. for store downloads,
    /// where COMPOSER is the songwriter's legal name).
    #[arg(long)]
    no_composers: bool,
    /// A file that's already stored only gets the new origin. Without this, what the
    /// import says that its track doesn't have yet (another album, differing metadata,
    /// covers, ids) is merged into it (an undoable merge). Quicker for libraries where
    /// the same file never comes twice.
    #[arg(long)]
    no_merge_duplicates: bool,
    /// Don't take the origin from a SoundCloud or store link in the COMMENT (see the
    /// README): `--origin` is used for files whose path gives none, e.g. files that
    /// aren't from the page they link to.
    #[arg(long)]
    no_comment_links: bool,
    /// Put every new track on the Review page with this note, to fix later (e.g.
    /// `--flag "imported quickly: check title and credits"`).
    #[arg(long, value_name = "NOTE")]
    flag: Option<String>,
    /// Start at file N, as the progress shows it (`[N/total]`), e.g. to go on after an
    /// interrupted run. The same folder, filters and flags give the same order.
    #[arg(long, value_name = "N")]
    resume_from: Option<usize>,
    /// Work out and print what would happen, without changing anything.
    #[arg(long)]
    dry_run: bool,
    /// Send the files to the Acetate at this URL (e.g. `http://nas:3000`) instead of
    /// importing them here: the same folder reading, sent over HTTP (`POST /api/ingest`).
    /// Needs the environment variable ACETATE_TOKEN (a session token, from
    /// `POST /api/auth/login`). No database is needed here.
    #[arg(long)]
    server: Option<String>,
    /// Only files whose path (relative to <dir>) matches one of these globs, e.g.
    /// `--include '**/FLACs/**'` (repeatable). `*` matches across folders too.
    #[arg(long)]
    include: Vec<String>,
    /// Leave out files whose path (relative to <dir>) matches one of these globs, e.g.
    /// `--exclude '*.aif'` (repeatable). Wins over --include.
    #[arg(long)]
    exclude: Vec<String>,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum LayoutArg {
    Netease,
    Qq,
    Kuwo,
    Soundcloud,
    Instrumentals,
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

    // `acetate ingest --server`: a client of another Acetate, so no database or storage
    // (and no configuration) is needed here.
    if let Command::Ingest(args) = &cli.command
        && let Some(server) = &args.server
    {
        let token = std::env::var("ACETATE_TOKEN")
            .context("--server needs ACETATE_TOKEN: a session token (the `token` from POST /api/auth/login)")?;
        let server = ingest_client::Server::connect(server, &token).await?;
        let options = ingest_options(args, Groups::Remote(&server)).await?;
        return ingest_command::run(ingest_command::Target::Server(server), &args.dir, options).await;
    }
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

    // The catalog's change log keeps 90 days (clients with an older copy get the whole
    // catalog); see migration 0029.
    {
        let mut conn = pool.acquire().await?;
        let trimmed = db::library::trim_changes(&mut conn, 90).await?;
        if trimmed > 0 {
            tracing::info!("trimmed {trimmed} old catalog changes");
        }
    }

    let store = BlobStore::open_volumes(&config.volumes)?;
    check_volumes(&pool, &store).await?;

    match cli.command {
        Command::Serve => serve(config, pool, store).await,
        Command::User { command } => user_commands::run(&pool, command).await,
        Command::Import {
            paths,
            origin,
            status,
            sources,
            restrict_to,
            volume,
        } => {
            let volume = cli_volume(&store, volume)?;
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
                tags_from: None,
                volume,
            };
            import(pool, store, paths, options).await
        }
        Command::Ingest(args) => {
            let options = ingest_options(&args, Groups::Local(&pool, &store)).await?;
            ingest_command::run(ingest_command::Target::Local { pool: &pool, store: &store }, &args.dir, options).await
        }
        Command::Verify { dir, blobs, list_ok, list_missing } => match (dir, blobs) {
            (None, true) => verify_command::verify_blobs(&pool, &store).await,
            (Some(dir), false) => {
                let options = verify_command::Options { list_ok, list_missing };
                verify_command::verify_dir(&pool, &store, &dir, options).await
            }
            _ => anyhow::bail!("give either a folder to check, or --blobs"),
        },
        Command::MergeDuplicates { max_length_difference_ms, ignore_case, dry_run } => {
            let options = merge_duplicates_command::Options { max_length_difference_ms, ignore_case, dry_run };
            merge_duplicates_command::run(&pool, options).await
        }
        Command::DuplicateAlbums {
            max_track_difference,
            max_length_difference_ms,
            min_title_similarity,
            any_artists,
            merge,
        } => {
            let options = duplicate_albums_command::Options {
                max_track_difference,
                max_length_difference_ms,
                min_title_similarity,
                any_artists,
                merge,
            };
            duplicate_albums_command::run(&pool, options).await
        }
        Command::FixMergedCovers { dry_run } => duplicate_albums_command::fix_merged_covers(&pool, dry_run).await,
        Command::ImportNavidrome { db, path_maps, libraries, password_key, dry_run } => {
            let path_maps = path_maps
                .iter()
                .map(|m| navidrome::parse_path_map(m))
                .collect::<anyhow::Result<Vec<_>>>()?;
            let libraries = libraries.iter().map(|l| navidrome::parse_library(l)).collect();
            let options = navidrome::Options { db, path_maps, libraries, password_key, dry_run };
            navidrome::run(&pool, options).await
        }
        Command::ReplaceFile {
            track_id,
            path,
            reason,
            origin,
            volume,
        } => {
            let volume = cli_volume(&store, volume)?;
            let origin = parse_source(&origin)?;
            let pending =
                ingest::receive_path(&store, &path, volume.as_deref().unwrap_or(store.default_volume())).await?;
            let options = ReplaceOptions {
                user: None, // the CLI acts as the system
                uploaded_filename: file_name(&path),
                origin,
                reason,
                volume,
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

        let volume = options.volume.as_deref().unwrap_or(store.default_volume());
        let result = match ingest::receive_path(&store, &path, volume).await {
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

/// Where `acetate ingest` looks up group names (`--restrict-to`) and volumes.
enum Groups<'a> {
    /// This machine's database and storage.
    Local(&'a sqlx::PgPool, &'a BlobStore),
    /// The server `--server` sends to (volumes are checked there).
    Remote(&'a ingest_client::Server),
}

/// `acetate ingest`'s options from its arguments.
async fn ingest_options(args: &IngestArgs, groups: Groups<'_>) -> anyhow::Result<ingest_command::Options> {
    use crate::ingest::layouts::{id_folders, soundcloud, ytdlp, Layout};
    let volume = match groups {
        Groups::Local(_, store) => cli_volume(store, args.volume.clone())?,
        Groups::Remote(_) => args.volume.clone(),
    };
    let layout = match (args.layout, args.platform) {
        (LayoutArg::Netease, None) => Layout::IdFolders(id_folders::Platform::Netease),
        (LayoutArg::Qq, None) => Layout::IdFolders(id_folders::Platform::Qq),
        (LayoutArg::Kuwo, None) => Layout::IdFolders(id_folders::Platform::Kuwo),
        (LayoutArg::Tags, None) => Layout::Tags,
        (LayoutArg::Soundcloud, None) => Layout::Soundcloud,
        (LayoutArg::Instrumentals, None) => Layout::Instrumentals,
        (LayoutArg::Ytdlp, Some(PlatformArg::Soundcloud)) => Layout::Ytdlp(ytdlp::Platform::Soundcloud),
        (LayoutArg::Ytdlp, Some(PlatformArg::Youtube)) => Layout::Ytdlp(ytdlp::Platform::Youtube),
        (LayoutArg::Ytdlp, None) => anyhow::bail!("--layout ytdlp needs --platform"),
        (_, Some(_)) => anyhow::bail!("--platform is only for --layout ytdlp"),
    };
    let origin = match &args.origin {
        Some(json) => Some(parse_source(json)?),
        None if layout == Layout::Tags => {
            anyhow::bail!("--layout tags needs --origin: where did the files come from?")
        }
        None => None,
    };
    let restrict_to = match (&args.restrict_to, groups) {
        (Some(names), Groups::Local(pool, _)) => Some(group_ids(pool, names).await?),
        (Some(names), Groups::Remote(server)) => Some(server.group_ids(&group_names(names)).await?),
        (None, _) => None,
    };
    // Every song is in the map, and without it songs outside album folders would get
    // no album id: so these layouts never run without it.
    let mut traktrain = None;
    let (id_map, sc_map) = match (&args.id_map, layout) {
        (Some(path), Layout::IdFolders(platform)) => (Some(id_folders::IdMap::load(platform, path)?), None),
        (Some(path), Layout::Soundcloud) => (None, Some(soundcloud::ScMap::load(path)?)),
        (Some(path), Layout::Instrumentals) => {
            traktrain = Some(crate::ingest::layouts::instrumentals::TraktrainMap::load(path)?);
            (None, None)
        }
        (Some(_), _) => anyhow::bail!("--id-map is only for --layout netease, qq, kuwo, soundcloud or instrumentals"),
        (None, Layout::IdFolders(_)) => {
            anyhow::bail!("--layout netease/qq/kuwo needs --id-map <map.json> (the song id -> album map)")
        }
        (None, Layout::Soundcloud) => anyhow::bail!("--layout soundcloud needs --id-map <lookup.json>"),
        (None, _) => (None, None),
    };
    // Covers named by album id: only NetEase's come like that (QQ's are by song).
    if args.covers.is_some() && layout != Layout::IdFolders(id_folders::Platform::Netease) {
        anyhow::bail!("--covers is only for --layout netease");
    }
    if args.tag_fallback && id_map.is_none() {
        anyhow::bail!("--tag-fallback is only for --layout netease, qq or kuwo");
    }
    Ok(ingest_command::Options {
        layout,
        origin,
        covers: args.covers.clone(),
        id_map,
        sc_map,
        traktrain,
        flag: args.flag.clone().filter(|f| !f.trim().is_empty()),
        status: args.status.clone(),
        restrict_to,
        volume,
        reflink_only: args.reflink_only,
        tag_fallback: args.tag_fallback,
        lax_artist_names: args.lax_artist_name,
        no_composers: args.no_composers,
        no_duplicate_merge: args.no_merge_duplicates,
        comment_links: !args.no_comment_links,
        resume_from: args.resume_from,
        filter: ingest_command::PathFilter::new(&args.include, &args.exclude)?,
        dry_run: args.dry_run,
    })
}

/// A `--volume` given on the command line, checked against the configured ones.
fn cli_volume(store: &BlobStore, volume: Option<String>) -> anyhow::Result<Option<String>> {
    if let Some(name) = &volume {
        if !store.has_volume(name) {
            let names: Vec<&str> = store.volumes().iter().map(|v| v.name.as_str()).collect();
            anyhow::bail!("no storage volume called \"{name}\" (ACETATE_VOLUMES has: {})", names.join(", "));
        }
    }
    Ok(volume)
}

/// Every volume the database has blobs on must be configured here (ACETATE_VOLUMES), or
/// those blobs couldn't be found: e.g. a database restored on another machine whose
/// configuration doesn't name the HDD volume yet.
async fn check_volumes(pool: &sqlx::PgPool, store: &BlobStore) -> anyhow::Result<()> {
    let mut tx = db::begin(pool, None).await?;
    let in_use = db::blobs::volumes_in_use(&mut tx).await?;
    let missing: Vec<String> = in_use
        .iter()
        .filter(|(name, _, _)| !store.has_volume(name))
        .map(|(name, blobs, _)| format!("{name} ({blobs} blobs)"))
        .collect();
    if !missing.is_empty() {
        anyhow::bail!(
            "the database has blobs on volumes that aren't configured: {}. Add them to \
             ACETATE_VOLUMES (name=/path), e.g. ACETATE_VOLUMES=main=/srv/acetate,archive=/mnt/hdd/acetate",
            missing.join(", ")
        );
    }
    Ok(())
}

/// Comma-separated group names -> their ids. Empty = no groups (admins only).
/// `--restrict-to friends, family` → the names.
fn group_names(names: &str) -> Vec<String> {
    names.split(',').map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect()
}

async fn group_ids(pool: &sqlx::PgPool, names: &str) -> anyhow::Result<Vec<Uuid>> {
    let names = group_names(names);
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
