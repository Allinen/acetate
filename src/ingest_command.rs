//! `acetate ingest <dir> --layout ...`: imports a whole folder, using what the folder
//! layout says about each file (platform ids) to match artists and albums. See
//! `ingest::layouts` and the README, "Automated ingestion". Videos in it (music videos)
//! become extras of kind "video", named after their files and linked to nothing yet (link
//! them from their pages, or Review's "Unlinked extras"); one an extra has already (the same
//! bytes) is left out.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use globset::{Glob, GlobSet, GlobSetBuilder};
use sqlx::PgPool;

use crate::db;
use crate::ingest::describe::{AlbumHint, Hints, MatchedBy, Report, ReportStatus};
use crate::ingest::layouts::{id_folders, instrumentals, soundcloud, Layout};
use crate::ingest::{self, comment_links, video, IngestOptions};
use crate::ingest_client::Server;
use crate::model::sources::ValidSource;
use crate::storage::{BlobStore, PendingBlob};

/// Files with these extensions are imported as tracks, and videos (`video::EXTENSIONS`) as
/// extras (anything else, like cover images, is skipped). An `.mp4` is either.
const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "m4a", "mp4", "aac", "alac", "opus", "ogg", "oga", "wav", "aif", "aiff", "wv",
    "ape",
];

/// The reason logged on the extras videos become.
pub(crate) const IMPORTED: &str = "imported from disk";

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
    /// The Traktrain lookup, for `--layout instrumentals` (see `instrumentals::TraktrainMap`).
    pub traktrain: Option<instrumentals::TraktrainMap>,
    /// Put every new track on the Review page with this note (`--flag`).
    pub flag: Option<String>,
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
    /// See `Hints::no_composers`.
    pub no_composers: bool,
    /// See `Hints::no_duplicate_merge`.
    pub no_duplicate_merge: bool,
    /// Take the origin from a link in the comment when the path gives none (see
    /// `comment_links`). Off: `--origin` instead.
    pub comment_links: bool,
    /// Start at this file (1-based, as the progress shows it: `[8726/12984]`), after
    /// filtering and sorting. The ones before are left out as if done.
    pub resume_from: Option<usize>,
    /// `--include` / `--exclude`.
    pub filter: PathFilter,
    /// `--videos`: only the videos; the audio is left out.
    pub videos_only: bool,
    pub dry_run: bool,
}

#[derive(Default)]
struct Totals {
    created: usize,
    duplicates: usize,
    same_song: usize,
    skipped: usize,
    failed: usize,
    /// Videos made extras, and left out (an extra has them already).
    videos: usize,
    videos_stored: usize,
    /// Of `failed`, videos.
    videos_failed: usize,
    /// `--videos`: `.mp4` files that are just sound, left out.
    not_videos: usize,
    /// How the files got into the store: reflinked (no space used) or copied.
    reflinked: usize,
    copied: usize,
}

/// Which paths `--include` / `--exclude` let through: relative to the folder given, with
/// `/` between folders. No includes: everything that isn't excluded.
pub struct PathFilter {
    include: Option<GlobSet>,
    exclude: Option<GlobSet>,
}

impl PathFilter {
    pub fn new(include: &[String], exclude: &[String]) -> anyhow::Result<PathFilter> {
        let set = |patterns: &[String]| -> anyhow::Result<Option<GlobSet>> {
            if patterns.is_empty() {
                return Ok(None);
            }
            let mut builder = GlobSetBuilder::new();
            for pattern in patterns {
                builder.add(Glob::new(pattern).with_context(|| format!("bad glob {pattern:?}"))?);
            }
            Ok(Some(builder.build()?))
        };
        Ok(PathFilter { include: set(include)?, exclude: set(exclude)? })
    }

    pub fn allows(&self, relative: &Path) -> bool {
        let text = relative.to_string_lossy().replace('\\', "/");
        self.include.as_ref().is_none_or(|set| set.is_match(&text))
            && !self.exclude.as_ref().is_some_and(|set| set.is_match(&text))
    }
}

/// Where the files go.
pub enum Target<'a> {
    /// This Acetate: its database and storage.
    Local { pool: &'a PgPool, store: &'a BlobStore },
    /// Another Acetate, over HTTP (`--server`).
    Server(Server),
}

/// What became of one file.
pub(crate) enum Outcome {
    Done(Report),
    /// Refused (e.g. missing tags): listed on the Review page (on a real run).
    Skipped(String),
    Failed(String),
    /// A video made an extra (None: a dry run), and what to say about it.
    VideoAdded { extra_id: Option<uuid::Uuid>, note: Option<String> },
    /// A video an extra has already.
    VideoStored(db::extras::StoredAs),
}

pub async fn run(target: Target<'_>, dir: &Path, options: Options) -> anyhow::Result<()> {
    // Absolute, so the layout sees the folder's real name even for `.`. A single file
    // counts as a folder holding just it.
    let given = std::fs::canonicalize(dir).with_context(|| format!("opening {}", dir.display()))?;
    let (root, mut files) = if given.is_file() {
        let parent = given.parent().map(Path::to_path_buf).unwrap_or_else(|| given.clone());
        (parent, vec![given])
    } else {
        let mut files = Vec::new();
        find_files(&given, options.videos_only, &mut files)?;
        (given, files)
    };
    let found = files.len();
    files.retain(|path| options.filter.allows(path.strip_prefix(&root).unwrap_or(path)));
    files.sort();
    let covers = match &options.covers {
        Some(dir) => cover_index(dir)?,
        None => HashMap::new(),
    };
    println!(
        "{} {} in {}{}{}",
        files.len(),
        if options.videos_only { "video files" } else { "audio and video files" },
        root.display(),
        if files.len() < found { format!(" ({} left out by --include/--exclude)", found - files.len()) } else { String::new() },
        match (&target, options.dry_run) {
            (_, false) => String::new(),
            (Target::Local { .. }, true) => " (dry run: nothing will be changed)".to_string(),
            // Each file is its own request, so it can't see what earlier ones would have
            // created: an artist new to the server is "new" for every file.
            (Target::Server(_), true) => " (dry run, file by file: nothing will be changed; artists and albums \
                                          new to the server show as new for every file)"
                .to_string(),
        }
    );

    // A local dry run is one transaction for the whole folder, rolled back at the end, so
    // a file sees the artists and albums earlier files would have created. Each file gets
    // a savepoint, so one that fails doesn't undo the others. A real run commits file by
    // file instead: stopping halfway keeps what's done, and running again skips it
    // (duplicates).
    let mut dry_run_tx = match (&target, options.dry_run) {
        (Target::Local { pool, .. }, true) => Some(db::begin(pool, None).await?),
        _ => None,
    };
    let mut totals = Totals::default();

    // Fetches covers by URL, set up when the first one is needed.
    let mut artworks: Option<soundcloud::Artworks> = None;

    // `--resume-from N`: the numbering stays the same as a full run's, so the progress
    // lines match the interrupted run's.
    let start = options.resume_from.unwrap_or(1);
    if start == 0 || (start > files.len() && !files.is_empty()) {
        anyhow::bail!("--resume-from {start}: there are files 1 to {}", files.len());
    }
    if start > 1 {
        println!("resuming at [{start}/{}]: the {} before it are left out", files.len(), start - 1);
    }

    for (index, path) in files.iter().enumerate() {
        if index + 1 < start {
            continue;
        }
        // With the position, so a long run shows how far along it is.
        let shown = format!("[{}/{}] {}", index + 1, files.len(), path.strip_prefix(&root).unwrap_or(path).display());
        // A video is an extra, whatever the layout says.
        if video::is_video_file(path).await {
            let outcome = match &target {
                Target::Local { pool, store } => add_video(pool, store, path, &options, &mut totals, &shown).await?,
                Target::Server(server) => server.send_video(path, options.restrict_to.as_deref(), options.dry_run).await?,
            };
            if matches!(outcome, Outcome::Failed(_)) {
                totals.videos_failed += 1;
            }
            tally(&mut totals, &shown, outcome, &[]);
            continue;
        }
        if options.videos_only {
            totals.not_videos += 1;
            println!("not video  {shown}: just sound, left out (import it without --videos)");
            continue;
        }
        let mut info = options.layout.read(&root, path);
        if let (true, None, Some(map)) = (options.tag_fallback, &info.track, &options.id_map) {
            // Unreadable tags: no title or artists to go by (ingest reports the file).
            let extension = path.extension().and_then(|e| e.to_str());
            let tags = ingest::tags::read_tags(path, extension).ok();
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
        if let Some(map) = &options.traktrain {
            map.apply(path, &mut info);
        }
        // Nothing from the path: a SoundCloud or store link in the comment says where the
        // file is from (see `comment_links`), before --origin.
        if info.track.is_none() && options.comment_links {
            let extension = path.extension().and_then(|e| e.to_str());
            let comment = crate::ingest::tags::read_tags(path, extension).ok().and_then(|t| t.comment);
            if let Some(origin) = comment.as_deref().and_then(comment_links::origin_from_comment) {
                let what = match (origin.data["song_permalinks"][0].as_str(), origin.data["url"].as_str()) {
                    (Some(permalink), _) => format!("SoundCloud {permalink}"),
                    (None, Some(url)) => format!("{} {url}", origin.data["label"].as_str().unwrap_or("?")),
                    _ => origin.platform.clone(),
                };
                info.warnings.push(format!("origin from the link in the comment: {what}"));
                info.track = Some(origin);
            }
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
                no_composers: options.no_composers,
                no_duplicate_merge: options.no_duplicate_merge,
                given: info.given,
                flag: options.flag.clone(),
                described: None,
            },
            local_path: Some(path.display().to_string()),
            edited_after: false,
            tags_from: None,
            volume: options.volume.clone(),
        };

        let (pool, store) = match &target {
            Target::Local { pool, store } => (*pool, *store),
            Target::Server(server) => {
                let outcome = server.send(path, &ingest_options, options.dry_run).await?;
                tally(&mut totals, &shown, outcome, &info.warnings);
                continue;
            }
        };
        let volume = options.volume.as_deref().unwrap_or(store.default_volume());
        let result = match take_in(store, path, &options, &mut totals, &shown).await? {
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

        let outcome = match result {
            Ok(report) => Outcome::Done(report),
            Err(err) if ingest::is_refusal(&err) => Outcome::Skipped(err.to_string()),
            Err(err) => Outcome::Failed(format!("{err:#}")),
        };
        tally(&mut totals, &shown, outcome, &info.warnings);
    }

    if let Some(tx) = dry_run_tx {
        tx.rollback().await?;
    } else if !options.dry_run && totals.skipped + totals.failed - totals.videos_failed > 0 {
        // (Videos that failed aren't: there's nothing to fix in them there.)
        println!("\nAudio files that didn't import are listed on the Review page (with their audio).");
    }
    println!();
    if !options.videos_only {
        println!(
            "{} created ({} with the same song id as an existing track, flagged), {} duplicates, \
             {} skipped, {} failed",
            totals.created, totals.same_song, totals.duplicates, totals.skipped, totals.failed - totals.videos_failed,
        );
    }
    if options.videos_only || totals.videos + totals.videos_stored + totals.videos_failed > 0 {
        let not_videos = match totals.not_videos {
            0 => String::new(),
            n => format!(", {n} just sound (left out)"),
        };
        println!(
            "videos: {} made extras, {} stored already, {} failed{not_videos}",
            totals.videos, totals.videos_stored, totals.videos_failed
        );
    }
    match &target {
        Target::Local { .. } => println!(
            "files taken in: {} reflinked (no extra space), {} copied",
            totals.reflinked, totals.copied
        ),
        Target::Server(server) => println!("sent to {}", server.url()),
    }
    if options.dry_run {
        println!("dry run: nothing was changed");
    }
    if totals.failed > 0 {
        anyhow::bail!("{} files failed", totals.failed);
    }
    Ok(())
}

/// Prints what became of a file and counts it.
fn tally(totals: &mut Totals, shown: &str, outcome: Outcome, path_warnings: &[String]) {
    match outcome {
        Outcome::Done(report) => {
            print_report(shown, &report, path_warnings);
            match report.status {
                ReportStatus::Created => totals.created += 1,
                ReportStatus::Duplicate => totals.duplicates += 1,
            }
            if !report.same_song_as.is_empty() {
                totals.same_song += 1;
            }
        }
        Outcome::Skipped(reason) => {
            totals.skipped += 1;
            eprintln!("skipped    {shown}: {reason}");
        }
        Outcome::Failed(reason) => {
            totals.failed += 1;
            eprintln!("error      {shown}: {reason}");
        }
        Outcome::VideoAdded { extra_id, note } => {
            totals.videos += 1;
            let id = extra_id.map(|id| id.to_string()).unwrap_or_default();
            println!("video      {id}  {shown}");
            if let Some(note) = note {
                println!("    {note}");
            }
        }
        Outcome::VideoStored(stored) => {
            totals.videos_stored += 1;
            println!("duplicate  {}  {shown}", stored.extra_id);
            println!("    video: {} has it already", stored.described());
        }
    }
}

/// A file taken into the store: a reflink if possible; otherwise a copy, unless
/// --reflink-only (then the run stops). Err inside: this file couldn't be.
async fn take_in(store: &BlobStore, path: &Path, options: &Options, totals: &mut Totals, shown: &str) -> anyhow::Result<anyhow::Result<PendingBlob>> {
    let volume = options.volume.as_deref().unwrap_or(store.default_volume());
    Ok(match store.reflink_in(path, volume).await {
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
    })
}

/// A video, as an extra linked to nothing yet (seen by `--restrict-to`'s groups, if given),
/// unless an extra has the same bytes already. Committed on its own (a dry run only looks).
async fn add_video(pool: &PgPool, store: &BlobStore, path: &Path, options: &Options, totals: &mut Totals, shown: &str) -> anyhow::Result<Outcome> {
    let pending = match take_in(store, path, options, totals, shown).await? {
        Ok(pending) => pending,
        Err(err) => return Ok(Outcome::Failed(format!("{err:#}"))),
    };
    let result = async {
        let (sha256, size) = (pending.sha256.clone(), pending.size as i64);
        let mut tx = db::begin(pool, None).await?;
        if let Some(stored) = db::extras::stored_as(&mut tx, &sha256).await? {
            return Ok(Outcome::VideoStored(stored));
        }
        if options.dry_run {
            return Ok(Outcome::VideoAdded { extra_id: None, note: None });
        }
        let volume = options.volume.as_deref().unwrap_or(store.default_volume());
        let (volume, stored_at) = store.commit(pending, volume).await?;
        db::blobs::register(&mut tx, &sha256, &volume, Some(size)).await?;
        let name = file_name(path);
        let mut facts = video::examine(store, &stored_at, &name).await;
        facts.imported_from = Some(path.to_string_lossy().to_string());
        let title = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| name.clone());
        let access = options.restrict_to.clone().map(|groups| db::extras::Access { restricted: true, groups });
        let (extra, _) = db::extras::create(&mut tx, &title, "video", &Default::default(), &[], access, IMPORTED).await?;
        db::extras::add_file(&mut tx, extra, &sha256, &name, size, &facts, IMPORTED).await?;
        tx.commit().await?;
        let note = match (facts.width, facts.height) {
            (Some(w), Some(h)) => format!("{w}×{h}"),
            _ => "! size not read (is ffprobe installed?)".into(),
        };
        anyhow::Ok(Outcome::VideoAdded { extra_id: Some(extra), note: Some(note) })
    }
    .await;
    Ok(result.unwrap_or_else(|err| Outcome::Failed(format!("{err:#}"))))
}

fn print_report(shown: &str, report: &Report, path_warnings: &[String]) {
    let id = report.track_id.map(|id| id.to_string()).unwrap_or_default();
    match report.status {
        ReportStatus::Created => println!("created    {id}  {shown}"),
        ReportStatus::Duplicate => println!("duplicate  {id}  {shown}"),
    }
    if report.status == ReportStatus::Duplicate && report.added.is_empty() {
        if report.album_covers_added > 0 {
            println!("    +{} album cover", report.album_covers_added);
        }
        return;
    }
    // A stored file imported again, with something new: merged into its track.
    for added in &report.added {
        println!("    + {added}");
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

/// All audio and video files below `dir` (or only videos, by their names), recursively.
fn find_files(dir: &Path, videos_only: bool, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            find_files(&path, videos_only, out)?;
        } else if video::is_video_name(&path.to_string_lossy()) || (!videos_only && is_audio(&path)) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn include_and_exclude_globs() {
        let only_flacs = PathFilter::new(&["**/FLACs/**".into()], &["*.aif".into()]).unwrap();
        assert!(only_flacs.allows(Path::new("1001776/FLACs/Singles/a - 1 - x.wav")));
        assert!(!only_flacs.allows(Path::new("1001776/streams/Singles/a - 1 - x.m4a")));
        assert!(!only_flacs.allows(Path::new("100001739/FLACs/Albums/1/01 - a - 1 - x.aif")), "excluded wins");
        let everything = PathFilter::new(&[], &[]).unwrap();
        assert!(everything.allows(Path::new("anything.flac")));
        // `*` crosses folders.
        let wavs = PathFilter::new(&["*.wav".into()], &[]).unwrap();
        assert!(wavs.allows(Path::new("a/b/c.wav")));
        assert!(PathFilter::new(&["[".into()], &[]).is_err());
    }
}
