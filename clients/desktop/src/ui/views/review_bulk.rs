//! Review's bulk tools ("Bulk…" or `b`): for the chosen chip's kind, what's shown in the
//! list (the filter applies: `artist:505CASH source:youtube` with "possible duplicate"
//! chosen is those tracks' duplicate flags only, their other flags left open).
//!
//! * Any kind: mark the shown flags of that kind done, with one note.
//! * Ambiguous artist: the flags by the name in the tags, each name credited to one of its
//!   candidates on all its tracks at once (`POST /api/review/ambiguous`: a revision each).
//! * No album: each track gets a single of its own (`POST /api/tracks/singles`).
//! * Possible duplicate: the pairs with the same title and about the same length are
//!   merged (`POST /api/review/merge-duplicates`), after a preview of exactly which.
//!
//! What it'll act on is taken when the popup opens and listed in full first; nothing
//! happens before the button.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText, Vec2};
use serde::Deserialize;
use uuid::Uuid;

use crate::catalog::Catalog;
use crate::review::Flag;
use crate::ui::theme::{font, semibold, PALETTE as P};
use crate::ui::widgets::length;
use crate::ui::{thousands, Action, Shell};

/// How many one request names (the server takes up to 10,000; smaller shows progress).
const BATCH: usize = 500;

/// An ambiguous name, and its flags.
struct Name {
    name: String,
    flags: Vec<Uuid>,
    /// Every candidate its flags name: (id, name, disambiguation).
    candidates: Vec<(Uuid, String, String)>,
    pick: Option<Uuid>,
    /// What the server did (or its error).
    outcome: Option<Result<String, String>>,
}

#[derive(Debug, Default, Deserialize)]
struct Settled {
    settled: usize,
    changed: usize,
    skipped: usize,
}

#[derive(Debug, Default, Deserialize)]
struct Singles {
    made: Vec<serde_json::Value>,
    skipped: Vec<Uuid>,
}

/// A track of a pair, as `POST /api/review/merge-duplicates` answers.
#[derive(Debug, Deserialize)]
struct PairTrack {
    title: String,
    duration_ms: Option<i64>,
    codec: Option<String>,
    size_bytes: i64,
}

#[derive(Debug, Deserialize)]
struct Pair {
    kept: PairTrack,
    merged: PairTrack,
    merged_file_kept: bool,
    /// Only for those that failed.
    #[serde(default)]
    error: String,
}

#[derive(Debug, Deserialize)]
struct Merged {
    merged: Vec<Pair>,
    different: usize,
    failed: Vec<Pair>,
}

/// How the pairs are told the same (what a preview was made with).
#[derive(Debug, Clone, Copy, PartialEq)]
struct SameTrack {
    max_length_difference_ms: i64,
    ignore_case: bool,
}

/// A preview or a real run, and what it was made with.
struct MergeRun {
    same: SameTrack,
    dry_run: bool,
    result: Result<Merged, String>,
}

/// The server takes at most this many flags at once; more need a narrower filter.
const MERGE_LIMIT: usize = 10_000;

enum Answer {
    Name(usize, Result<Settled, String>),
    Singles(Result<Singles, String>),
    Merge(MergeRun),
}

pub struct ReviewBulk {
    kind: String,
    kind_name: String,
    /// The shown tracks (catalog places) and their open flags of the kind.
    tracks: Vec<u32>,
    flags: Vec<Uuid>,
    /// Ambiguous artists: the names (most flags first), and flags with no details yet.
    names: Vec<Name>,
    unknown: usize,
    resolution: String,
    reason: String,
    sending: bool,
    /// How many of those sent are done (singles).
    progress: Arc<AtomicUsize>,
    answer: Arc<Mutex<Vec<Answer>>>,
    singles: Option<Result<String, String>>,
    /// Possible duplicates: how to tell the same track, and the last preview or run.
    same: SameTrack,
    merge: Option<MergeRun>,
}

impl ReviewBulk {
    /// For `kind`, the shown `tracks` (catalog places): their open flags of that kind now.
    pub fn open(shell: &Shell, catalog: &Catalog, kind: &str, tracks: Vec<u32>, filter: &str) -> ReviewBulk {
        let vocab = shell.account.vocab.clone().unwrap_or_default();
        let kind_name = vocab.review_kind_name(kind).to_string();
        let done = &shell.flags.done;
        let flags: Vec<Uuid> = tracks
            .iter()
            .flat_map(|&i| catalog.issues.get(&catalog.tracks[i as usize].id).into_iter().flatten())
            .filter(|(id, k)| k == kind && !done.contains(id))
            .map(|(id, _)| *id)
            .collect();
        let (names, unknown) = if kind == "ambiguous_artist" { names(&shell.flags.list, &flags) } else { (Vec::new(), 0) };
        let filter = filter.trim();
        let resolution = if filter.is_empty() { format!("{kind_name}: marked done in bulk") } else { format!("{kind_name}: marked done in bulk ({filter})") };
        ReviewBulk {
            kind: kind.to_string(),
            kind_name,
            tracks,
            flags,
            names,
            unknown,
            resolution,
            reason: String::new(),
            sending: false,
            progress: Arc::default(),
            answer: Arc::default(),
            singles: None,
            same: SameTrack { max_length_difference_ms: 1_000, ignore_case: false },
            merge: None,
        }
    }

    /// The popup. Returns true when it's closed.
    pub fn show(&mut self, shell: &mut Shell, catalog: &Catalog) -> bool {
        let answers = std::mem::take(&mut *self.answer.lock().unwrap());
        for answer in answers {
            self.sending = false;
            // A preview changed nothing.
            if !matches!(answer, Answer::Merge(MergeRun { dry_run: true, .. })) {
                shell.act(Action::RefreshLibrary);
            }
            match answer {
                Answer::Name(n, result) => {
                    self.names[n].outcome = Some(result.map(|s| {
                        let mut text = format!("{} settled, {} tracks credited anew", thousands(s.settled), thousands(s.changed));
                        if s.skipped > 0 {
                            text.push_str(&format!(", {} left open (edited since, or settled already)", thousands(s.skipped)));
                        }
                        text
                    }))
                }
                Answer::Merge(run) => self.merge = Some(run),
                Answer::Singles(result) => {
                    self.singles = Some(result.map(|s| {
                        let mut text = format!("{} singles made", thousands(s.made.len()));
                        if !s.skipped.is_empty() {
                            text.push_str(&format!(", {} tracks left as they were (on an album already)", thousands(s.skipped.len())));
                        }
                        text
                    }))
                }
            }
        }
        shell.modal_open = true;
        let ctx = shell.ctx.clone();
        let title = format!("{}: {} flags shown", self.kind_name, thousands(self.flags.len()));
        let mut resolve = false;
        let mut singles = false;
        let mut settle: Option<usize> = None;
        let mut merge: Option<bool> = None;
        let mut cancel = false;
        let closed = crate::ui::modal::show(&ctx, "review-bulk", &title, Vec2::new(900.0, 760.0), |ui| {
            if self.flags.is_empty() {
                ui.label(RichText::new("None shown: nothing to do.").color(P.muted));
                return;
            }
            if self.kind == "ambiguous_artist" {
                self.ambiguous(ui, catalog, &mut settle);
                ui.add_space(10.0);
            }
            if self.kind == "possible_duplicate" {
                self.duplicates(ui, &mut merge);
                ui.add_space(10.0);
            }
            if self.kind == "missing_album" {
                ui.label(RichText::new("Make them singles").font(font(14.0, semibold())));
                ui.label(format!(
                    "Each of the {} tracks gets a single of its own: named after it, credited to its primary artists, with its release date and covers; the track on it as disc 1, track 1 (a revision of each). Its “no album” flag settles itself.",
                    thousands(self.tracks.len())
                ));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text("Why (optional: kept with every change)").desired_width(380.0));
                    singles = ui.add_enabled(!self.sending && self.singles.is_none(), egui::Button::new(format!("Make {} singles", thousands(self.tracks.len())))).clicked();
                });
                if self.sending {
                    let done = self.progress.load(Ordering::Relaxed);
                    ui.label(RichText::new(format!("Working… {} of {}", thousands(done), thousands(self.tracks.len()))).color(P.muted));
                }
                match &self.singles {
                    Some(Ok(text)) => {
                        ui.label(RichText::new(text.as_str()).color(P.accent));
                    }
                    Some(Err(err)) => {
                        ui.label(RichText::new(err.as_str()).color(P.danger));
                    }
                    None => {}
                }
                ui.add_space(10.0);
            }
            ui.label(RichText::new("Mark them done").font(font(14.0, semibold())));
            ui.label(format!(
                "The {} “{}” flags of the tracks below are marked done, with this note. Their other flags stay open.",
                thousands(self.flags.len()),
                self.kind_name
            ));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.resolution).desired_width(520.0));
                resolve = ui.add_enabled(!self.sending, egui::Button::new(format!("Mark {} done", thousands(self.flags.len())))).clicked();
                cancel = ui.button("Close").clicked();
            });
            ui.add_space(10.0);
            ui.label(RichText::new(format!("The tracks ({})", thousands(self.tracks.len()))).font(font(14.0, semibold())));
            let row = 20.0;
            egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row, self.tracks.len(), |ui, range| {
                for n in range {
                    let t = &catalog.tracks[self.tracks[n] as usize];
                    let artist = if t.artist_display.is_empty() { "(no artist)" } else { t.artist_display.as_str() };
                    ui.label(format!("{}  ·  {artist}", t.title));
                }
            });
        });
        if resolve {
            shell.flags.resolve(&shell.api, shell.runtime.handle(), &self.flags, self.resolution.trim());
            shell.act(Action::RefreshLibrary);
            return true;
        }
        if singles {
            self.make_singles(shell, catalog);
        }
        if let Some(n) = settle {
            self.settle(shell, n);
        }
        if let Some(dry_run) = merge {
            self.merge_duplicates(shell, dry_run);
        }
        (closed || cancel) && !self.sending
    }

    /// Ambiguous artists: a row per name, its candidates to choose from, and the button.
    fn ambiguous(&mut self, ui: &mut egui::Ui, catalog: &Catalog, settle: &mut Option<usize>) {
        ui.label(RichText::new("Credit one artist per name").font(font(14.0, semibold())));
        ui.label("Each name from the tags, with the artists it matched. The one chosen is credited on all its tracks (in place of whichever of them is credited now), a revision each, and the flags settled.");
        if self.unknown > 0 {
            ui.label(RichText::new(format!("{} flags aren't loaded yet (their names unknown): left out.", thousands(self.unknown))).color(P.faint).small());
        }
        ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text("Why (optional: kept with every change)").desired_width(380.0));
        egui::ScrollArea::vertical().id_salt("review-bulk-names").max_height(260.0).show(ui, |ui| {
            for (n, name) in self.names.iter_mut().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("“{}”", name.name)).font(font(14.0, semibold())));
                    ui.label(RichText::new(format!("{} tracks", thousands(name.flags.len()))).color(P.muted));
                    let chosen = name.pick.and_then(|id| name.candidates.iter().find(|c| c.0 == id)).map(|c| label(catalog, c)).unwrap_or_else(|| "which one?".into());
                    egui::ComboBox::from_id_salt(("review-bulk-name", n)).selected_text(chosen).width(300.0).show_ui(ui, |ui| {
                        for c in &name.candidates {
                            if ui.selectable_label(name.pick == Some(c.0), label(catalog, c)).clicked() {
                                name.pick = Some(c.0);
                            }
                        }
                    });
                    let ready = name.pick.is_some() && !self.sending && !matches!(name.outcome, Some(Ok(_)));
                    if ui.add_enabled(ready, egui::Button::new(format!("Credit it on all {}", thousands(name.flags.len())))).clicked() {
                        *settle = Some(n);
                    }
                    match &name.outcome {
                        Some(Ok(text)) => {
                            ui.label(RichText::new(text.as_str()).color(P.accent));
                        }
                        Some(Err(err)) => {
                            ui.label(RichText::new(err.as_str()).color(P.danger));
                        }
                        None => {}
                    }
                });
            }
        });
    }

    /// Possible duplicates: the options, a preview of the pairs, and the button. `merge`:
    /// Some(dry run?) when one is asked for.
    fn duplicates(&mut self, ui: &mut egui::Ui, merge: &mut Option<bool>) {
        ui.label(RichText::new("Merge the exact duplicates").font(font(14.0, semibold())));
        ui.label("Of the pairs these flags name, those with the same title and about the same length are merged: the older track is kept (its metadata, its covers first), with the better of the two files (WAV over FLAC over MP3, then the bigger). Pairs that differ stay flagged. Each merge can be undone on the kept track's page.");
        if self.flags.len() > MERGE_LIMIT {
            ui.label(RichText::new(format!("More than {} flags shown: filter to fewer.", thousands(MERGE_LIMIT))).color(P.danger));
            return;
        }
        ui.horizontal(|ui| {
            ui.label("Lengths at most");
            ui.add(egui::DragValue::new(&mut self.same.max_length_difference_ms).range(0..=10_000).speed(50).suffix(" ms"));
            ui.label("apart");
            ui.add_space(12.0);
            ui.checkbox(&mut self.same.ignore_case, "Titles may differ in case (“Intro”, “INTRO”)");
        });
        // A preview stands for these options only, and a real run is done.
        let previewed = self.merge.as_ref().is_some_and(|m| m.dry_run && m.same == self.same && m.result.is_ok());
        let ran = self.merge.as_ref().is_some_and(|m| !m.dry_run && m.result.is_ok());
        let count = self.merge.as_ref().and_then(|m| m.result.as_ref().ok()).map(|m| m.merged.len()).unwrap_or(0);
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text("Why (optional: kept with every merge)").desired_width(380.0));
            if ui.add_enabled(!self.sending && !ran, egui::Button::new("Preview")).clicked() {
                *merge = Some(true);
            }
            let text = format!("Merge {} pairs", thousands(count));
            if ui.add_enabled(!self.sending && previewed && count > 0, egui::Button::new(text)).clicked() {
                *merge = Some(false);
            }
        });
        if self.sending {
            ui.label(RichText::new("Working…").color(P.muted));
        }
        let Some(run) = &self.merge else { return };
        let done = match &run.result {
            Ok(done) => done,
            Err(err) => {
                ui.label(RichText::new(err.as_str()).color(P.danger));
                return;
            }
        };
        let (merged, failed) = if run.dry_run { ("would be merged", "would fail") } else { ("merged", "failed") };
        let mut text = format!("{} pairs {merged}, {} left as they are (title or length differs)", thousands(done.merged.len()), thousands(done.different));
        if !done.failed.is_empty() {
            text.push_str(&format!(", {} {failed}", thousands(done.failed.len())));
        }
        if run.dry_run && run.same != self.same {
            text.push_str(" — options changed since: preview again");
        }
        ui.label(RichText::new(text).color(if run.dry_run { P.muted } else { P.accent }));
        egui::ScrollArea::vertical().id_salt("review-bulk-pairs").max_height(240.0).show(ui, |ui| {
            for pair in &done.merged {
                ui.label(pair_text(pair));
            }
            for pair in &done.failed {
                ui.label(RichText::new(format!("{}: {}", pair_text(pair), pair.error)).color(P.danger));
            }
        });
    }

    fn merge_duplicates(&mut self, shell: &Shell, dry_run: bool) {
        self.sending = true;
        let same = self.same;
        let (api, answer, ctx, flags, reason) = (shell.api.clone(), self.answer.clone(), shell.ctx.clone(), self.flags.clone(), self.reason.trim().to_string());
        shell.runtime.spawn(async move {
            let body = serde_json::json!({
                "flag_ids": flags,
                "max_length_difference_ms": same.max_length_difference_ms,
                "ignore_case": same.ignore_case,
                "reason": reason,
                "dry_run": dry_run,
            });
            let result = api.send_json::<Merged>(reqwest::Method::POST, "/api/review/merge-duplicates", Some(body)).await.map_err(|err| err.message);
            answer.lock().unwrap().push(Answer::Merge(MergeRun { same, dry_run, result }));
            ctx.request_repaint();
        });
    }

    fn settle(&mut self, shell: &Shell, n: usize) {
        let Some(artist) = self.names[n].pick else { return };
        self.sending = true;
        self.names[n].outcome = None;
        let (api, answer, ctx, flags, reason) = (shell.api.clone(), self.answer.clone(), shell.ctx.clone(), self.names[n].flags.clone(), self.reason.trim().to_string());
        shell.runtime.spawn(async move {
            let mut all = Settled::default();
            let mut result = Ok(());
            for batch in flags.chunks(BATCH) {
                let body = serde_json::json!({ "flag_ids": batch, "artist_id": artist, "reason": reason });
                match api.send_json::<Settled>(reqwest::Method::POST, "/api/review/ambiguous", Some(body)).await {
                    Ok(s) => {
                        all.settled += s.settled;
                        all.changed += s.changed;
                        all.skipped += s.skipped;
                    }
                    Err(err) => {
                        result = Err(format!("{} settled, then: {}", all.settled, err.message));
                        break;
                    }
                }
            }
            answer.lock().unwrap().push(Answer::Name(n, result.map(|_| all)));
            ctx.request_repaint();
        });
    }

    fn make_singles(&mut self, shell: &Shell, catalog: &Catalog) {
        self.sending = true;
        self.progress.store(0, Ordering::Relaxed);
        let tracks: Vec<Uuid> = self.tracks.iter().map(|&i| catalog.tracks[i as usize].id).collect();
        let (api, answer, ctx, progress, reason) = (shell.api.clone(), self.answer.clone(), shell.ctx.clone(), self.progress.clone(), self.reason.trim().to_string());
        shell.runtime.spawn(async move {
            let mut all = Singles::default();
            let mut result = Ok(());
            for batch in tracks.chunks(BATCH) {
                let body = serde_json::json!({ "track_ids": batch, "reason": reason });
                match api.send_json::<Singles>(reqwest::Method::POST, "/api/tracks/singles", Some(body)).await {
                    Ok(s) => {
                        all.made.extend(s.made);
                        all.skipped.extend(s.skipped);
                        progress.fetch_add(batch.len(), Ordering::Relaxed);
                        ctx.request_repaint();
                    }
                    Err(err) => {
                        result = Err(format!("{} singles made, then: {}", all.made.len(), err.message));
                        break;
                    }
                }
            }
            answer.lock().unwrap().push(Answer::Singles(result.map(|_| all)));
            ctx.request_repaint();
        });
    }
}

/// A pair as listed: the title(s), both lengths, and which file stays (of the two).
fn pair_text(pair: &Pair) -> String {
    let title = if pair.kept.title == pair.merged.title { format!("“{}”", pair.kept.title) } else { format!("“{}” ← “{}”", pair.kept.title, pair.merged.title) };
    let file = |t: &PairTrack| format!("{} {:.1} MB", t.codec.as_deref().unwrap_or("?"), t.size_bytes as f64 / 1_000_000.0);
    let (kept, other) = if pair.merged_file_kept { (&pair.merged, &pair.kept) } else { (&pair.kept, &pair.merged) };
    format!("{title}  ·  {} / {}  ·  keeps {} (not {})", length(pair.kept.duration_ms), length(pair.merged.duration_ms), file(kept), file(other))
}

/// A candidate as listed: its name, disambiguation and how many tracks it has.
fn label(catalog: &Catalog, (id, name, dis): &(Uuid, String, String)) -> String {
    let size = catalog.artist(*id).map(|a| format!(" · {} tracks", thousands(a.track_count as usize))).unwrap_or_default();
    // As the artist is now (the flag's copy is from when the flags were fetched).
    let label = catalog.artist_label(*id).unwrap_or_else(|| if dis.is_empty() { name.clone() } else { format!("{name} ({dis})") });
    format!("{label}{size}")
}

/// The ambiguous-artist `flags` by name (most flags first), from the details `list` has;
/// and how many aren't in it.
fn names(list: &[Flag], flags: &[Uuid]) -> (Vec<Name>, usize) {
    let by_id: HashMap<Uuid, &Flag> = list.iter().map(|f| (f.id, f)).collect();
    let mut names: Vec<Name> = Vec::new();
    let mut unknown = 0;
    for id in flags {
        let Some(flag) = by_id.get(id) else {
            unknown += 1;
            continue;
        };
        let name = flag.related.get("name").and_then(|n| n.as_str()).unwrap_or("?").to_string();
        let at = match names.iter().position(|n| n.name == name) {
            Some(at) => at,
            None => {
                names.push(Name { name, flags: Vec::new(), candidates: Vec::new(), pick: None, outcome: None });
                names.len() - 1
            }
        };
        names[at].flags.push(*id);
        for artist in flag.related_artists.as_array().into_iter().flatten() {
            let Some(id) = artist.get("id").and_then(|v| v.as_str()).and_then(|v| v.parse::<Uuid>().ok()) else { continue };
            if names[at].candidates.iter().any(|c| c.0 == id) {
                continue;
            }
            let text = |key: &str| artist.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string();
            names[at].candidates.push((id, text("name"), text("disambiguation")));
        }
    }
    names.sort_by_key(|n| std::cmp::Reverse(n.flags.len()));
    (names, unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag(id: u8, name: &str, artists: &[(u8, &str)]) -> Flag {
        let uuid = |n: u8| format!("00000000-0000-7000-8000-0000000000{n:02x}");
        serde_json::from_value(serde_json::json!({
            "id": uuid(id), "kind": "ambiguous_artist", "track_id": uuid(id + 100),
            "related": {"name": name},
            "related_artists": artists.iter().map(|(n, a)| serde_json::json!({"id": uuid(*n), "name": a})).collect::<Vec<_>>(),
            "imported_from": null,
        }))
        .unwrap()
    }

    #[test]
    fn ambiguous_flags_go_by_name() {
        let list = [flag(1, "Kai", &[(10, "Kai"), (11, "KAI")]), flag(2, "Nova", &[(12, "Nova")]), flag(3, "Kai", &[(11, "KAI"), (13, "kai")])];
        let ids: Vec<Uuid> = list.iter().map(|f| f.id).chain([Uuid::max()]).collect();
        let (names, unknown) = names(&list, &ids);
        assert_eq!(unknown, 1);
        assert_eq!(names.iter().map(|n| (n.name.as_str(), n.flags.len())).collect::<Vec<_>>(), [("Kai", 2), ("Nova", 1)]);
        assert_eq!(names[0].candidates.iter().map(|c| c.1.as_str()).collect::<Vec<_>>(), ["Kai", "KAI", "kai"], "each once, in order");
    }
}
