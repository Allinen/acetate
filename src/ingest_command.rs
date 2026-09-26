//! `acetate ingest <dir> --layout ...`: imports a whole folder, using what the folder
//! layout says about each file (platform ids) to match artists and albums. See
//! `ingest::layouts` and the README, "Automated ingestion".

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use sqlx::PgPool;

use crate::db;
use crate::ingest::describe::{AlbumHint, Hints, MatchedBy, Report, ReportStatus};
use crate::ingest::layouts::{id_folders, soundcloud, Layout};
use crate::ingest::{self, IngestOptions};
use crate::model::sources::ValidSource;
use crate::storage::BlobStore;

/// Files with these extensions are imported (anything else, like cover images, is skipped).
const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "m4a", "mp4", "aac", "alac", "opus", "ogg", "oga", "wav", "aif", "aiff", "wv",
    "ape",
];

pub struct Options {
    pub layout: Layout,
    /// Where files came from when the path doesn't say (e.g. no id in the name).
    pub origin: Option<ValidSource>,
    /// A folder of album covers named `<title> - <album id>.<ext>` (NetEase layout).
    pub covers: Option<PathBuf>,
    /// Songs' albums, positions and other ids (see `id_folders::IdMap`).
    pub id_map: Option<id_folders::IdMap>,
    /// The SoundCloud lookup (see `soundcloud::ScMap`).
    pub sc_map: Option<soundcloud::ScMap>,
    /// For tracks whose status the layout doesn't say.
    pub status: String,
    pub restrict_to: Option<Vec<uuid::Uuid>>,
    /// Which storage volume the audio goes to (None: the default one).
    pub volume: Option<String>,
    /// Stop instead of copying a file that can't be reflinked (so a big import can't
    /// fill the disk by accident).
    pub reflink_only: bool,
    /// Files without a song id in the name: look for it in the id map by their tags (see
    /// `IdMap::fall_back_to_tags`).
    pub tag_fallback: bool,
    /// See `describe::lax_artist_names`.
    pub lax_artist_names: bool,
    pub dry_run: bool,
}

#[derive(Default)]
struct Totals {
    created: usize,
    duplicates: usize,
    same_song: usize,
    skipped: usize,
    failed: usize,
    /// How the files got into the store: reflinked (no space used) or copied.
    reflinked: usize,
    copied: usize,
}

pub async fn run(pool: &PgPool, store: &BlobStore, dir: &Path, options: Options) -> anyhow::Result<()> {
    // Absolute, so the layout sees the folder's real name even for `.`.
    let root = std::fs::canonicalize(dir).with_context(|| format!("opening {}", dir.display()))?;
    let mut files = Vec::new();
    find_audio_files(&root, &mut files)?;
    files.sort();
    let covers = match &options.covers {
        Some(dir) => cover_index(dir)?,
        None => HashMap::new(),
    };
    println!(
        "{} audio files in {}{}",
        files.len(),
        root.display(),
        if options.dry_run { " (dry run: nothing will be changed)" } else { "" }
    );

    // A dry run is one transaction for the whole folder, rolled back at the end, so a
    // file sees the artists and albums earlier files would have created. Each file gets a
    // savepoint, so one that fails doesn't undo the others. A real run commits file by file
    // instead: stopping halfway keeps what's done, and running again skips it (duplicates).
    let mut dry_run_tx = match options.dry_run {
        true => Some(db::begin(pool, None).await?),
        false => None,
    };
    let mut totals = Totals::default();

    // Fetches covers by URL, set up when the first one is needed.
    let mut artworks: Option<soundcloud::Artworks> = None;

    for (index, path) in files.iter().enumerate() {
        // With the position, so a long run shows how far along it is.
        let shown = format!("[{}/{}] {}", index + 1, files.len(), path.strip_prefix(&root).unwrap_or(path).display());
        let mut info = options.layout.read(&root, path);
        if let (true, None, Some(map)) = (options.tag_fallback, &info.track, &options.id_map) {
            // Unreadable tags: no title or artists to go by (ingest reports the file).
            let extension = path.extension().and_then(|e| e.to_str());
            let tags = crate::ingest::tags::read_tags(path, extension).ok();
            let title = tags.as_ref().and_then(|t| t.title.clone());
            let artists = tags.map(|t| t.artists).unwrap_or_default();
            map.fall_back_to_tags(&mut info, title.as_deref(), &artists);
        }
        if let Some(map) = &options.id_map {
            map.apply(&mut info);
        }
        if let Some(map) = &options.sc_map {
            map.apply(&mut info);
        }
        let Some(origin) = info.track.clone().or_else(|| options.origin.clone()) else {
            totals.skipped += 1;
            eprintln!("skipped    {shown}: {} and no --origin given", info.warnings.join("; "));
            continue;
        };
        let mut album = AlbumHint {
            sources: info.album_sources.clone(),
            track_number: info.album_track_number,
            single: info.album_is_single,
            release_date: info.album_release_date.clone(),
            ..Default::default()
        };
        if let Some(cover) = info.album_cover_key.as_ref().and_then(|key| covers.get(key)) {
            let bytes = std::fs::read(cover).with_context(|| format!("reading {}", cover.display()))?;
            album.covers.push(bytes);
        }
        // Covers by URL (SoundCloud): a saved copy, else downloaded.
        let mut track_covers = Vec::new();
        if !info.track_cover_urls.is_empty() || !info.album_cover_urls.is_empty() {
            let artworks = match &mut artworks {
                Some(a) => a,
                None => artworks.insert(soundcloud::Artworks::new()?),
            };
            let folder = info.cover_folder.as_deref();
            track_covers = artworks.get_all(&info.track_cover_urls, folder, &mut info.warnings).await;
            album.covers.extend(artworks.get_all(&info.album_cover_urls, folder, &mut info.warnings).await);
        }
        let has_album_hint = !album.sources.is_empty()
            || !album.covers.is_empty()
            || album.track_number.is_some()
            || album.single.is_some()
            || album.release_date.is_some();
        let ingest_options = IngestOptions {
            user: None, // the CLI acts as the system
            uploaded_filename: file_name(path),
            origin,
            status: info.status.clone().unwrap_or_else(|| options.status.clone()),
            sources: Vec::new(),
            restrict_to: options.restrict_to.clone(),
            hints: Hints {
                artists: info.artists,
                album: has_album_hint.then_some(album),
                lax_artist_names: options.lax_artist_names,
                release_date: info.release_date,
                release_date_over_tags: info.release_date_over_tags,
                first_artist_sources: info.first_artist_sources,
                track_covers,
                skip_embedded_covers: info.skip_embedded_covers,
                original_filename: info.original_filename,
            },
            local_path: Some(path.display().to_string()),
            given: Default::default(),
            volume: options.volume.clone(),
        };

        let volume = options.volume.as_deref().unwrap_or(store.default_volume());
        // A reflink if possible; otherwise a copy, unless --reflink-only.
        let received = match store.reflink_in(path, volume).await {
            Ok(Some(pending)) => {
                totals.reflinked += 1;
                Ok(pending)
            }
            Ok(None) if options.reflink_only => anyhow::bail!(
                "can't reflink {shown} into volume {volume} (not the same filesystem and mount as the volume?); \
                 stopping because of --reflink-only. Files before it are done; running again continues here."
            ),
            Ok(None) => {
                totals.copied += 1;
                ingest::copy_path(store, path).await
            }
            Err(err) => Err(err),
        };
        let result = match received {
            Err(err) => Err(err),
            Ok(pending) => match &mut dry_run_tx {
                None => ingest::ingest_with_report(pool, store, pending, &ingest_options, false).await,
                Some(tx) => {
                    let file = ingest::StagedFile::new(store, pending, true, volume).await?;
                    let mut savepoint = sqlx::Connection::begin(&mut **tx).await?;
                    match ingest::ingest_in(&mut savepoint, store, &file, &ingest_options, true).await {
                        Ok(report) => {
                            savepoint.commit().await?;
                            Ok(report)
                        }
                        Err(err) => {
                            savepoint.rollback().await?;
                            Err(ingest::as_rejected(err))
                        }
                    }
                }
            },
        };

        match result {
            Ok(report) => {
                print_report(&shown, &report, &info.warnings);
                match report.status {
                    ReportStatus::Created => totals.created += 1,
                    ReportStatus::Duplicate => totals.duplicates += 1,
                }
                if !report.same_song_as.is_empty() {
                    totals.same_song += 1;
                }
            }
            Err(err) if ingest::is_refusal(&err) => {
                totals.skipped += 1;
                eprintln!("skipped    {shown}: {err}");
            }
            Err(err) => {
                totals.failed += 1;
                eprintln!("error      {shown}: {err:#}");
            }
        }
    }

    if let Some(tx) = dry_run_tx {
        tx.rollback().await?;
    } else if totals.skipped + totals.failed > 0 {
        println!("\nFiles that didn't import are listed on the Review page (with their audio).");
    }
    println!(
        "\n{} created ({} with the same song id as an existing track, flagged), {} duplicates, \
         {} skipped, {} failed\nfiles taken in: {} reflinked (no extra space), {} copied{}",
        totals.created,
        totals.same_song,
        totals.duplicates,
        totals.skipped,
        totals.failed,
        totals.reflinked,
        totals.copied,
        if options.dry_run { "\ndry run: nothing was changed" } else { "" }
    );
    if totals.failed > 0 {
        anyhow::bail!("{} files failed", totals.failed);
    }
    Ok(())
}

fn print_report(shown: &str, report: &Report, path_warnings: &[String]) {
    let id = report.track_id.map(|id| id.to_string()).unwrap_or_default();
    match report.status {
        ReportStatus::Created => println!("created    {id}  {shown}"),
        ReportStatus::Duplicate => println!("duplicate  {id}  {shown}"),
    }
    if report.status == ReportStatus::Duplicate {
        if report.album_covers_added > 0 {
            println!("    +{} album cover", report.album_covers_added);
        }
        return;
    }
    let artists: Vec<String> = report
        .artists
        .iter()
        .map(|a| format!("{} ({})", a.name, matched(a.matched_by)))
        .collect();
    let mut line = format!("    artists: {}", artists.join(", "));
    if let Some(album) = &report.album {
        line += &format!(" | album: {} ({})", album.name, matched(album.matched_by));
    }
    if report.album_covers_added > 0 {
        line += &format!(" +{} album cover", report.album_covers_added);
    }
    println!("{line}");
    if !report.same_song_as.is_empty() {
        let ids: Vec<String> = report.same_song_as.iter().map(|id| id.to_string()).collect();
        println!("    ! same song id as {} (flagged for review)", ids.join(", "));
    }
    for warning in path_warnings.iter().chain(&report.warnings) {
        println!("    ! {warning}");
    }
}

fn matched(by: MatchedBy) -> &'static str {
    match by {
        MatchedBy::Id => "given id",
        MatchedBy::PlatformId => "matched by platform id",
        MatchedBy::Name => "matched by name",
        MatchedBy::Created => "new",
    }
}

/// All audio files below `dir`, recursively.
fn find_audio_files(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            find_audio_files(&path, out)?;
        } else if is_audio(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.contains(&e.to_lowercase().as_str()))
}

/// Album covers in a folder, by album id: `Do This Thing Often - 10001702.jpg` → "10001702".
fn cover_index(dir: &Path) -> anyhow::Result<HashMap<String, PathBuf>> {
    let mut index = HashMap::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        if let Some((_, id)) = id_folders::split_id(&stem) {
            index.insert(id.to_string(), path);
        }
    }
    Ok(index)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}
