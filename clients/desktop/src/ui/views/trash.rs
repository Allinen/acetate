//! The Trash: what's been moved to the trash (covers, sources, old audio files), to take
//! back out, and, for admins, to purge for good once it's been there long enough (the
//! server's grace period, 7 days). Purging asks for a typed confirmation, as the server
//! does: for one item the start of its hash (a source: its id), for several how many.
//! Nothing is ever purged by itself: only by an admin, here or in the web UI.
//!
//! "Old files" (admins): files no track plays anymore (replaced by a better one, or left
//! from a merge), with their sizes: where merges and upgrades free space. Marked ones are
//! moved to the trash (and can be purged a week later).
//!
//! Your playlists in the trash (everyone has playlists, so everyone sees this page), each
//! taken back out with "Restore".
//!
//! Marking: the box in front of a row, or "Mark all shown".

use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText, Sense, Vec2};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::ui::selection::Selection;
use crate::ui::theme::{bold, font, semibold, PALETTE as P};
use crate::ui::{thousands, widgets, Action, Page, Shell};

#[derive(Debug, Clone, Deserialize)]
pub struct TrashItem {
    pub kind: String,
    pub id: Uuid,
    /// What it is: a file's name, a source's platform and id, "cover".
    pub what: String,
    pub owner_id: Option<Uuid>,
    /// "track", "album" or "artist".
    pub owner_kind: Option<String>,
    pub owner: Option<String>,
    pub sha256: Option<String>,
    pub size_bytes: Option<i64>,
    pub trashed_at: String,
    pub trashed_by: Option<String>,
    pub purgeable_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OldFile {
    pub id: Uuid,
    pub track_id: Uuid,
    pub track_title: String,
    pub original_filename: Option<String>,
    pub uploaded_filename: String,
    pub codec: Option<String>,
    /// In kbps.
    pub bitrate: Option<i32>,
    pub size_bytes: i64,
    pub replaced_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum Tab {
    #[default]
    Trash,
    OldFiles,
}

/// Old files asked for at a time.
const PAGE: usize = 100;

type Answer<T> = Arc<Mutex<Option<Result<T, String>>>>;

/// A playlist of yours in the trash (`GET /api/playlists?trash=true`).
#[derive(Clone, Deserialize)]
struct TrashedPlaylist {
    id: Uuid,
    name: String,
    mine: bool,
}

#[derive(Default)]
pub struct TrashPage {
    tab: Tab,
    playlists: Vec<TrashedPlaylist>,
    playlists_answer: Answer<Vec<TrashedPlaylist>>,
    items: Option<Result<Vec<TrashItem>, String>>,
    items_answer: Answer<Vec<TrashItem>>,
    /// Only this kind shown ("cover", "source", "file", "problem"); None: all.
    only: Option<String>,
    marked: Selection<Uuid>,
    /// Purge asked: what's typed to confirm it.
    purging: Option<String>,
    old: Vec<OldFile>,
    /// More old files to ask for (the last page was full).
    old_more: bool,
    old_loading: bool,
    old_answer: Answer<Vec<OldFile>>,
    old_marked: Selection<Uuid>,
    /// A restore, purge or trashing on its way, and what the last one said.
    busy: bool,
    done_answer: Answer<String>,
    result: Option<Result<String, String>>,
}

impl TrashPage {
    /// The page was opened: what's in the trash now (and the old files, for admins).
    pub fn opened(&mut self, shell: &Shell) {
        self.load(shell);
        if shell.account.can("admin") {
            self.old.clear();
            self.old_marked.clear();
            self.load_old(shell);
        }
    }

    fn load(&mut self, shell: &Shell) {
        let (api, answer, ctx) = (shell.api.clone(), self.playlists_answer.clone(), shell.ctx.clone());
        shell.runtime.spawn(async move {
            let result = api.send_json::<Vec<TrashedPlaylist>>(reqwest::Method::GET, "/api/playlists?trash=true", None).await.map_err(|err| err.message);
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
        // The rest of the trash is for who can put things there.
        if !may_trash_anything(shell) {
            self.items = Some(Ok(Vec::new()));
            return;
        }
        let (api, answer, ctx) = (shell.api.clone(), self.items_answer.clone(), shell.ctx.clone());
        shell.runtime.spawn(async move {
            let result = api.send_json::<Vec<TrashItem>>(reqwest::Method::GET, "/api/trash", None).await.map_err(|err| err.message);
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }

    /// The next page of old files (after the last one shown).
    fn load_old(&mut self, shell: &Shell) {
        self.old_loading = true;
        let before = self.old.last().map(|f| format!("&before={}", f.id)).unwrap_or_default();
        let path = format!("/api/files/replaced?limit={PAGE}{before}");
        let (api, answer, ctx) = (shell.api.clone(), self.old_answer.clone(), shell.ctx.clone());
        shell.runtime.spawn(async move {
            let result = api.send_json::<Vec<OldFile>>(reqwest::Method::GET, &path, None).await.map_err(|err| err.message);
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }

    /// Sends a restore, purge or trashing; `done` says what it did, from the answer.
    fn send(&mut self, shell: &Shell, path: &'static str, body: Value, done: impl FnOnce(&Value) -> String + Send + 'static) {
        self.busy = true;
        self.result = None;
        let (api, answer, ctx) = (shell.api.clone(), self.done_answer.clone(), shell.ctx.clone());
        shell.runtime.spawn(async move {
            let result = api.send_json::<Value>(reqwest::Method::POST, path, Some(body)).await.map(|v| done(&v)).map_err(|err| err.message);
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        if let Some(result) = self.playlists_answer.lock().unwrap().take() {
            match result {
                Ok(list) => self.playlists = list.into_iter().filter(|p| p.mine).collect(),
                Err(err) => self.result = Some(Err(format!("Couldn't list your playlists in the trash: {err}"))),
            }
        }
        if let Some(result) = self.items_answer.lock().unwrap().take() {
            // What's gone from the trash isn't marked anymore.
            if let Ok(items) = &result {
                self.marked.retain(|id| items.iter().any(|i| i.id == *id));
            }
            self.items = Some(result);
        }
        if let Some(result) = self.old_answer.lock().unwrap().take() {
            self.old_loading = false;
            match result {
                Ok(files) => {
                    self.old_more = files.len() == PAGE;
                    self.old.extend(files);
                }
                Err(err) => self.result = Some(Err(err)),
            }
        }
        // (Taken out first: the lock is let go at the end of this statement, before
        // `opened` below needs all of `self`.)
        let done = self.done_answer.lock().unwrap().take();
        if let Some(result) = done {
            self.busy = false;
            if result.is_ok() {
                self.marked.clear();
                self.purging = None;
                self.opened(shell);
                // A cover back (or gone) changes what tracks and albums show.
                shell.act(Action::RefreshLibrary);
            }
            self.result = Some(result);
        }

        ui.label(RichText::new("Trash").font(font(30.0, bold())));
        ui.add_space(8.0);
        let admin = shell.account.can("admin");
        if admin {
            ui.horizontal(|ui| {
                if widgets::chip(ui, "In the trash", self.tab == Tab::Trash) {
                    self.tab = Tab::Trash;
                }
                if widgets::chip(ui, "Old files no track plays", self.tab == Tab::OldFiles) {
                    self.tab = Tab::OldFiles;
                }
            });
            ui.add_space(8.0);
        }
        match &self.result {
            Some(Ok(done)) => {
                ui.label(RichText::new(done.as_str()).color(P.accent));
            }
            Some(Err(err)) => {
                ui.label(RichText::new(err.as_str()).color(P.danger));
            }
            None => {}
        }
        match self.tab {
            Tab::OldFiles if admin => self.old_files(ui, shell),
            _ => {
                self.playlists(ui, shell);
                if may_trash_anything(shell) {
                    self.trash(ui, shell, admin);
                }
            }
        }
    }

    /// Your playlists in the trash, each with "Restore".
    fn playlists(&mut self, ui: &mut egui::Ui, shell: &Shell) {
        if self.playlists.is_empty() {
            if !may_trash_anything(shell) {
                ui.label(RichText::new("No playlists of yours in the trash.").color(P.muted));
            }
            return;
        }
        ui.label(RichText::new("Your playlists").font(font(16.0, bold())));
        for playlist in self.playlists.clone() {
            ui.horizontal(|ui| {
                ui.label(&playlist.name);
                if ui.add_enabled(!self.busy, egui::Button::new("Restore").small()).clicked() {
                    self.busy = true;
                    self.result = None;
                    let (api, answer, ctx) = (shell.api.clone(), self.done_answer.clone(), shell.ctx.clone());
                    let (path, name) = (format!("/api/playlists/{}/restore", playlist.id), playlist.name.clone());
                    shell.runtime.spawn(async move {
                        let result = api.send(reqwest::Method::POST, &path, None).await.map(|()| format!("“{name}” is back")).map_err(|err| err.message);
                        *answer.lock().unwrap() = Some(result);
                        ctx.request_repaint();
                    });
                }
            });
        }
        ui.add_space(14.0);
    }

    fn trash(&mut self, ui: &mut egui::Ui, shell: &mut Shell, admin: bool) {
        let items = match &self.items {
            None => {
                ui.label(RichText::new("Loading…").color(P.faint));
                return;
            }
            Some(Err(err)) => {
                ui.label(RichText::new(format!("Couldn't load the trash: {err}")).color(P.danger));
                return;
            }
            Some(Ok(items)) => items.clone(),
        };
        if items.is_empty() {
            ui.label(RichText::new("The trash is empty.").color(P.muted));
            return;
        }
        let now = now_seconds();
        let purgeable = |i: &TrashItem| seconds(&i.purgeable_at).is_some_and(|at| at <= now);

        // ---- Kinds, and what's marked ----
        ui.horizontal_wrapped(|ui| {
            let total: i64 = items.iter().filter_map(|i| i.size_bytes).sum();
            if widgets::chip(ui, &format!("All  {} · {}", thousands(items.len()), size(total)), self.only.is_none()) {
                self.only = None;
            }
            for (kind, name) in [("cover", "Covers"), ("source", "Sources"), ("file", "Files"), ("extra", "Merged extras"), ("problem", "Didn't import")] {
                let count = items.iter().filter(|i| i.kind == kind).count();
                if count > 0 && widgets::chip(ui, &format!("{name}  {}", thousands(count)), self.only.as_deref() == Some(kind)) {
                    self.only = Some(kind.to_string());
                }
            }
        });
        let shown: Vec<&TrashItem> = items.iter().filter(|i| self.only.as_deref().is_none_or(|k| i.kind == k)).collect();
        let marked: Vec<&TrashItem> = items.iter().filter(|i| self.marked.contains(&i.id)).collect();
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if ui.small_button("Mark all shown").clicked() {
                self.marked.all(shown.len(), |n| shown.get(n).map(|i| i.id));
            }
            if !marked.is_empty() {
                let bytes: i64 = marked.iter().filter_map(|i| i.size_bytes).sum();
                ui.label(RichText::new(format!("{} marked · {}", thousands(marked.len()), size(bytes))).color(P.accent));
                if ui.small_button("Clear").clicked() {
                    self.marked.clear();
                    self.purging = None;
                }
                let can_restore = marked.iter().all(|i| may_trash(shell, &i.kind));
                let restore = ui.add_enabled(can_restore && !self.busy, egui::Button::new(format!("Take out of the trash ({})", marked.len())));
                if restore.on_disabled_hover_text("Some of them need a permission you haven't got").clicked() {
                    let body = json!({ "items": marked.iter().map(|i| json!({ "kind": i.kind, "id": i.id })).collect::<Vec<_>>() });
                    self.send(shell, "/api/trash/restore", body, |v| format!("Taken out of the trash: {}.", v["restored"].as_u64().unwrap_or(0)));
                }
                if admin && self.purging.is_none() {
                    let all_purgeable = marked.iter().all(|i| purgeable(i));
                    let purge = ui.add_enabled(all_purgeable && !self.busy, egui::Button::new(RichText::new(format!("Purge for good ({})…", marked.len())).color(P.danger)));
                    if purge.on_disabled_hover_text("Only what's been in the trash long enough (see each one's date)").clicked() {
                        self.purging = Some(String::new());
                    }
                }
            }
        });
        if let Some(typed) = self.purging.as_mut() {
            // What the server wants typed: one item, the start of its hash (a source: its
            // id); several, how many.
            let expected = match marked.as_slice() {
                [one] => one.sha256.clone().unwrap_or_else(|| one.id.to_string()),
                many => many.len().to_string(),
            };
            let asked = match marked.as_slice() {
                [one] if one.sha256.is_some() => "the first 6 characters of its hash (or more)".to_string(),
                [_] => "its id".to_string(),
                many => format!("how many there are ({})", many.len()),
            };
            let matches = if marked.len() == 1 { typed.trim().len() >= 6 && expected.starts_with(typed.trim()) } else { typed.trim() == expected };
            let mut go = false;
            let mut cancel = false;
            egui::Frame::NONE.fill(P.surface).stroke(egui::Stroke::new(1.0, P.danger)).corner_radius(8).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                ui.label(RichText::new("Purging deletes them for good: the images, files and snapshots are gone, and can't be brought back.").color(P.text));
                if let [one] = marked.as_slice()
                    && let Some(sha) = &one.sha256
                {
                    ui.label(RichText::new(format!("Its hash: {sha}")).monospace().small().color(P.muted));
                }
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("To go on, type {asked}:")).color(P.muted));
                    ui.add(egui::TextEdit::singleline(typed).desired_width(200.0));
                    go = ui.add_enabled(matches && !self.busy, egui::Button::new(RichText::new("Purge for good").color(P.danger))).clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
            if go {
                let confirm = typed.trim().to_string();
                let body = json!({ "items": marked.iter().map(|i| json!({ "kind": i.kind, "id": i.id })).collect::<Vec<_>>(), "confirm": confirm });
                self.send(shell, "/api/trash/purge", body, |v| {
                    format!("Purged {}, freeing {}.", v["purged"].as_u64().unwrap_or(0), size(v["bytes_freed"].as_i64().unwrap_or(0)))
                });
            }
            if cancel {
                self.purging = None;
            }
        }
        ui.add_space(8.0);

        // ---- The items, oldest first ----
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let ids: Vec<Uuid> = shown.iter().map(|i| i.id).collect();
            for (n, item) in shown.into_iter().enumerate() {
                ui.horizontal(|ui| {
                    let mut on = self.marked.contains(&item.id);
                    if ui.checkbox(&mut on, "").changed() {
                        // Shift+click: the range from the last one clicked.
                        let modifiers = ui.input(|i| i.modifiers);
                        self.marked.click(n, n, if modifiers.shift { modifiers } else { egui::Modifiers::COMMAND }, |n| ids.get(n).copied());
                        self.purging = None;
                    }
                    if item.kind == "cover" {
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
                        let cover = item.sha256.as_deref().and_then(crate::catalog::model::CoverKey::from_hex).map(|key| crate::catalog::model::Cover { id: item.id, key });
                        shell.cover(ui, cover, item.id, rect, 4, 0.0);
                    }
                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(format!("{} · {}", kind_name(&item.kind), item.what)).font(font(14.0, semibold())));
                            if let (Some(id), Some(kind), Some(name)) = (item.owner_id, item.owner_kind.as_deref(), item.owner.as_deref()) {
                                ui.label(RichText::new(if kind == "extra" { "into" } else { "of" }).color(P.faint));
                                let page = match kind {
                                    "extra" => Page::Extra(id),
                                    "album" => Page::Album(id),
                                    "artist" => Page::Artist(id),
                                    _ => Page::Track(id),
                                };
                                if widgets::link(ui, name, font(14.0, semibold()), P.muted) {
                                    shell.act(Action::Go(page));
                                }
                            }
                        });
                        let by = item.trashed_by.as_deref().map(|b| format!(" by {b}")).unwrap_or_default();
                        let bytes = item.size_bytes.map(|b| format!("{} · ", size(b))).unwrap_or_default();
                        let when = match seconds(&item.purgeable_at) {
                            Some(at) if at <= now => "can be purged".to_string(),
                            Some(at) => format!("can be purged in {}", days((at - now).max(0))),
                            None => String::new(),
                        };
                        ui.label(RichText::new(format!("{bytes}trashed {}{by} · {when}", day(&item.trashed_at))).color(P.faint).small());
                    });
                });
                ui.add_space(4.0);
            }
        });
    }

    fn old_files(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        ui.label(
            RichText::new("Files no track plays anymore: replaced by a better one, or left from a merge (each stays in its track's history). Moving them to the trash frees their space once they're purged, a week later.")
                .color(P.muted),
        );
        ui.add_space(6.0);
        // Their ids and sizes (copies: the buttons below change `self`).
        let marked: Vec<(Uuid, i64)> = self.old.iter().filter(|f| self.old_marked.contains(&f.id)).map(|f| (f.id, f.size_bytes)).collect();
        let marked_ids: Vec<Uuid> = marked.iter().map(|(id, _)| *id).collect();
        ui.horizontal_wrapped(|ui| {
            let total: i64 = self.old.iter().map(|f| f.size_bytes).sum();
            let more = if self.old_more { " so far" } else { "" };
            ui.label(RichText::new(format!("{} files · {}{more}", thousands(self.old.len()), size(total))).color(P.text));
            if ui.small_button("Mark all shown").clicked() {
                self.old_marked.all(self.old.len(), |n| self.old.get(n).map(|f| f.id));
            }
            if !marked.is_empty() {
                let bytes: i64 = marked.iter().map(|(_, size)| size).sum();
                ui.label(RichText::new(format!("{} marked · {}", thousands(marked.len()), size(bytes))).color(P.accent));
                if ui.small_button("Clear").clicked() {
                    self.old_marked.clear();
                }
                if ui.add_enabled(!self.busy, egui::Button::new(format!("Move {} to the trash", marked.len()))).clicked() {
                    self.send(shell, "/api/files/trash", json!({ "file_ids": marked_ids }), |v| {
                        format!("Moved to the trash: {}.", v["trashed"].as_u64().map(|n| n.to_string()).unwrap_or_else(|| "done".into()))
                    });
                    self.old_marked.clear();
                }
            }
        });
        ui.add_space(8.0);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let ids: Vec<Uuid> = self.old.iter().map(|f| f.id).collect();
            for (n, f) in self.old.iter().enumerate() {
                ui.horizontal(|ui| {
                    let mut on = self.old_marked.contains(&f.id);
                    if ui.checkbox(&mut on, "").changed() {
                        let modifiers = ui.input(|i| i.modifiers);
                        self.old_marked.click(n, n, if modifiers.shift { modifiers } else { egui::Modifiers::COMMAND }, |n| ids.get(n).copied());
                    }
                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            let name = f.original_filename.as_deref().unwrap_or(&f.uploaded_filename);
                            ui.label(RichText::new(name).font(font(14.0, semibold())));
                            ui.label(RichText::new("of").color(P.faint));
                            if widgets::link(ui, &f.track_title, font(14.0, semibold()), P.muted) {
                                shell.act(Action::Go(Page::Track(f.track_id)));
                            }
                        });
                        let quality = [f.codec.clone(), f.bitrate.map(|b| format!("{b} kbps"))].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                        ui.label(RichText::new(format!("{} · {quality} · replaced {}", size(f.size_bytes), day(&f.replaced_at))).color(P.faint).small());
                    });
                });
                ui.add_space(4.0);
            }
            if self.old.is_empty() && !self.old_loading {
                ui.label(RichText::new("None: every stored file is played by a track, or in the trash already.").color(P.muted));
            }
            if self.old_loading {
                ui.label(RichText::new("Loading…").color(P.faint));
            } else if self.old_more && ui.button(format!("Load {PAGE} more")).clicked() {
                self.load_old(shell);
            }
        });
    }
}

/// Whether the user may put anything but playlists in the trash (and so see it).
fn may_trash_anything(shell: &Shell) -> bool {
    shell.account.can("edit_metadata") || shell.account.can("edit_sources")
}

/// Whether the user may put this kind in the trash (and take it back out).
fn may_trash(shell: &Shell, kind: &str) -> bool {
    match kind {
        "cover" | "extra" => shell.account.can("edit_metadata"),
        "source" => shell.account.can("edit_sources"),
        "problem" => shell.account.can("upload"),
        _ => shell.account.can("admin"),
    }
}

fn kind_name(kind: &str) -> &str {
    match kind {
        "cover" => "Cover",
        "source" => "Source",
        "file" => "Audio file",
        "extra" => "Merged extra",
        "problem" => "Didn't import",
        other => other,
    }
}

use super::settings::size;

/// `2026-09-29` from a timestamp.
fn day(at: &str) -> &str {
    at.get(..10).unwrap_or(at)
}

/// "3 days", "5 hours".
fn days(secs: i64) -> String {
    match secs {
        s if s >= 2 * 86_400 => format!("{} days", s / 86_400),
        s if s >= 3600 => format!("{} hours", s / 3600),
        s => format!("{} minutes", (s / 60).max(1)),
    }
}

fn now_seconds() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Seconds since 1970 of an RFC 3339 time in UTC ("2026-09-29T10:51:07.1087Z"; the
/// server's are all UTC).
fn seconds(at: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| at.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    // Days since 1970-01-01 of a date (Howard Hinnant's `days_from_civil`).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_times_read_as_seconds() {
        assert_eq!(seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(seconds("2000-03-01T00:00:01.5Z"), Some(951_868_801));
        assert_eq!(seconds("2026-09-29T10:51:07.108720Z"), Some(1_790_679_067));
        assert_eq!(seconds("soon"), None);
        assert_eq!(days(3 * 86_400 + 5), "3 days");
    }

    /// Against a running server (a throwaway database: it moves a cover of the track to
    /// the trash and takes it back out; lists old files if the user is an admin):
    /// `ACETATE_TEST_SERVER=http://127.0.0.1:3002 ACETATE_TEST_TOKEN=… ACETATE_TEST_TRACK=<id>
    /// cargo test trash -- --ignored`
    #[test]
    #[ignore]
    fn a_cover_trashed_listed_and_taken_back_out() {
        use crate::api::Api;
        use crate::details::TrackDetail;
        let vars = ["ACETATE_TEST_SERVER", "ACETATE_TEST_TOKEN", "ACETATE_TEST_TRACK"].map(std::env::var);
        let [Ok(server), Ok(token), Ok(track)] = vars else { return };
        let dir = std::env::temp_dir().join(format!("acetate-trash-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), json!({"server": server, "token": token, "username": "test"}).to_string()).unwrap();
        let api = Api::new(dir);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let detail = api.send_json::<TrackDetail>(reqwest::Method::GET, &format!("/api/tracks/{track}"), None).await.unwrap();
            let cover = detail.covers.first().expect("the track has a cover").clone();
            api.send(reqwest::Method::DELETE, &format!("/api/covers/{}", cover.id), None).await.unwrap();

            let items: Vec<TrashItem> = api.send_json(reqwest::Method::GET, "/api/trash", None).await.unwrap();
            let item = items.iter().find(|i| i.id == cover.id).expect("in the trash");
            assert_eq!((item.kind.as_str(), item.owner_id, item.sha256.as_deref()), ("cover", Some(detail.id), Some(cover.blob_sha256.as_str())));
            assert!(seconds(&item.purgeable_at).unwrap() > now_seconds(), "not purgeable yet");

            let body = json!({ "items": [{ "kind": item.kind, "id": item.id }] });
            let restored: Value = api.send_json(reqwest::Method::POST, "/api/trash/restore", Some(body)).await.unwrap();
            assert_eq!(restored["restored"], 1);
            let items: Vec<TrashItem> = api.send_json(reqwest::Method::GET, "/api/trash", None).await.unwrap();
            assert!(items.iter().all(|i| i.id != cover.id));

            // Old files: read as the page reads them (admins only).
            if let Ok(files) = api.send_json::<Vec<OldFile>>(reqwest::Method::GET, "/api/files/replaced?limit=5", None).await {
                assert!(files.len() <= 5);
            }
        });
    }
}
