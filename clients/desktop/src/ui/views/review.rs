//! The Review page (6): the workbench for fixing the library, hands on the keyboard.
//!
//! Left, the tracks that need a look: every track with an open review flag, each saying
//! why. The flags are the server's, all kinds alike: those ingest and merges raise (fix
//! later, possible duplicate, ambiguous artist, artist merged) and those the server
//! raises itself for what a track is missing (no title, artists, album, cover, release
//! date), with their names from `/api/vocab`. Which tracks have which comes with the
//! library kept on this computer (`Catalog::issues`), so filtering and counting are
//! instant even for the whole library. One row of chips filters by kind (with counts);
//! text filters too. Group by folder, note, artist or kind. The first chip, "Didn't
//! import", shows the files the server refused or failed on instead (`not_imported`).
//!
//! Right, the selected track's form, with the cursor already in the first field that
//! needs fixing, and the file's name, folder and tags to fix from.
//!
//! Keys: Ctrl+Enter saves; then, if the server says nothing's missing anymore, "fix
//! later" is marked done and the next track comes. Ctrl+↓/↑ next/previous (anywhere);
//! outside text fields also j/k or ↓/↑, Space plays the track (or pauses it), u skips, d (Ctrl+D anywhere)
//! marks its flags done (of the chip's kind, if one's chosen). Ctrl+M compares a possible
//! duplicate side by side (to merge), and closes it again. b (or "Bulk…"), with a chip
//! chosen: the bulk tools for what's shown (`review_bulk`).
//!
//! Marking many, as in a track table (`marking`, the same tools): the box in front of a
//! row, `x`, or Ctrl+click marks or unmarks one; Shift+click or Shift+↑/↓ a range; Ctrl+A
//! everything shown; Esc clears. While any are marked, the change for all of them
//! (`edit::editor`) takes the place of the one track's form (Ctrl+E: the cursor in it;
//! Ctrl+Enter is its own then); Ctrl+M merges the two marked. Marking doesn't change
//! which track is selected (a plain click does): unmarking them all brings its form back.
//! A marked track that leaves the list for good (fixed, marked done, merged away) leaves
//! the marks too; one only hidden by a chip or the filter stays marked.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, Key, Modifiers, RichText, Sense, Vec2};
use uuid::Uuid;

use super::merge::{MergeView, Outcome};
use crate::account::Vocab;
use crate::catalog::Catalog;
use crate::playback::PlayerView;
use crate::review::Flag;
use crate::ui::theme::{bold, font, medium, semibold, PALETTE as P};
use crate::ui::{playing_track, widgets, Action, Shell};

const ROW: f32 = 46.0;
const LIST_WIDTH: f32 = 380.0;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum Group {
    #[default]
    Folder,
    Note,
    Artist,
    Kind,
    None,
}

impl Group {
    const ALL: [Group; 5] = [Group::Folder, Group::Note, Group::Artist, Group::Kind, Group::None];

    fn name(self) -> &'static str {
        match self {
            Group::Folder => "folder",
            Group::Note => "import note",
            Group::Artist => "artist",
            Group::Kind => "kind of flag",
            Group::None => "nothing",
        }
    }
}

/// A track in the list: its place in the catalog, the kinds of its open flags (in the
/// server's order), and the group it's in. (Copied, not positions in a flag list: those
/// change the moment a flag is marked done, before the rows are made again.)
struct Entry {
    track: u32,
    kinds: Vec<String>,
    group: String,
}

enum Row {
    Header(String, usize),
    Track(usize),
}

#[derive(Default)]
pub struct Review {
    group: Group,
    /// Only tracks with a flag of this kind (a chip); None: every track with any.
    only: Option<String>,
    /// `only` starts as "fix later" (the first time the page is shown).
    started: bool,
    filter: String,
    entries: Vec<Entry>,
    rows: Vec<Row>,
    /// How many tracks have each kind (before `only` narrows the list).
    counts: HashMap<String, usize>,
    /// What the list was built from: (catalog generation, flags version, filters).
    built_for: Option<(u64, u64, String)>,
    selected: Option<Uuid>,
    /// Where the selected track was in the list (to go on from there once it's left it).
    selected_at: usize,
    /// Scroll the list to the selected track.
    scroll_to_selected: bool,
    /// Put the line that fixes the first thing missing in the editor (once it's there).
    focus_first: bool,
    /// The line last prefilled for a flag: (the track, the flag's kind, the line).
    prefilled: Option<(Uuid, String, String)>,
    /// Tracks fixed or marked done here, this session.
    done: usize,
    notice: Option<String>,
    asked_flags: bool,
    /// Two tracks side by side, to merge (in place of the form).
    merge: Option<MergeView>,
    /// Pairs judged "not the same" here: (the flagged track, the other). A duplicate flag
    /// is settled once every track it names has been judged.
    not_same: std::collections::HashSet<(Uuid, Uuid)>,
    /// Tracks merged away here: gone from the list and from other flags' suggestions at
    /// once (the server's lists catch up a moment later).
    merged_away: std::collections::HashSet<Uuid>,
    /// Just applied: (the track, its automatic flags then, the library's generation then).
    /// What it's missing until the library catches up (the library's flags are older).
    fresh: Option<(Uuid, Vec<String>, u64)>,
    /// The bulk tools, open.
    bulk: Option<super::review_bulk::ReviewBulk>,
    /// Marked tracks, and the tools for them (as in a track table).
    marking: super::marking::Marking,
    /// Files that didn't import, and whether they're shown (in place of the list).
    not_imported: super::not_imported::NotImported,
    showing_not_imported: bool,
    /// Extras linked to nothing (videos imported from disk…), shown in place of the list.
    showing_unlinked: bool,
    /// Showing artists and albums nobody uses anymore instead of the flagged tracks.
    showing_unused: bool,
    asked_not_imported: bool,
}

impl Review {
    pub fn opened(&mut self) {
        self.asked_flags = false;
        self.asked_not_imported = false;
        self.focus_first = true;
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, player: &PlayerView) {
        if !shell.account.can("edit_metadata") {
            ui.label(RichText::new("Review is for fixing the library: it needs the edit_metadata permission.").color(P.muted));
            return;
        }
        if !self.started {
            self.started = true;
            self.only = Some("fix_later".into());
            self.marking.change_in_place();
        }
        for err in shell.flags.poll() {
            shell.controller.report(err);
        }
        if !self.asked_flags {
            self.asked_flags = true;
            shell.flags.refresh(&shell.api, shell.runtime.handle(), &shell.waker);
        }
        if !self.asked_not_imported {
            self.asked_not_imported = true;
            self.not_imported.refresh(shell);
        }
        self.not_imported.poll(shell);
        self.build(shell, catalog);
        // The bulk popup first: while it's open, the page's keys wait.
        if let Some(bulk) = &mut self.bulk
            && bulk.show(shell, catalog)
        {
            self.bulk = None;
        }
        // The marked tracks' popups, before the keys too.
        let entries = &self.entries;
        let id_at = |n: usize| entries.get(n).map(|e| catalog.track_at(e.track).id());
        self.marking.popups(&ui.ctx().clone(), shell, player, entries.len(), id_at, None);
        // (Files that didn't import have their own keys.)
        if !self.showing_not_imported && !self.showing_unlinked && !self.showing_unused {
            self.keys(ui.ctx(), shell, catalog, player);
        }
        // A key may have marked a flag done: the rows follow at once.
        self.build(shell, catalog);
        let vocab = shell.account.vocab.clone().unwrap_or_default();

        // ---- Top: title, filters ----
        ui.horizontal(|ui| {
            ui.label(RichText::new("Review").font(font(28.0, bold())));
            ui.add_space(12.0);
            let hint = if self.showing_not_imported { "/  filter: words in the name, folder, reason or tags" } else { "/  filter: words, or artist:… album:… year:… status:… flag:…" };
            ui.add(egui::TextEdit::singleline(&mut self.filter).id(egui::Id::new(super::FILTER_ID)).hint_text(hint).desired_width(260.0));
            egui::ComboBox::from_id_salt("review-group").selected_text(format!("group by {}", self.group.name())).show_ui(ui, |ui| {
                for group in Group::ALL {
                    ui.selectable_value(&mut self.group, group, group.name());
                }
            });
            if shell.flags.loading {
                ui.label(RichText::new("checking for new flags…").color(P.faint).small());
            } else if let Some(err) = &shell.flags.error {
                ui.label(RichText::new(format!("couldn't check for new flags: {err}")).color(P.danger).small());
            }
        });
        ui.add_space(6.0);
        // A chip per kind of flag (the server's order), with how many tracks have one.
        ui.horizontal_wrapped(|ui| {
            let mut kinds: Vec<String> = vocab.review_kinds.iter().map(|k| k.kind.clone()).collect();
            let mut unknown: Vec<String> = self.counts.keys().filter(|k| !kinds.contains(k)).cloned().collect();
            unknown.sort();
            kinds.extend(unknown);
            // Files that didn't import: not tracks, so shown in place of the list.
            let files = self.not_imported.matching(&self.filter);
            // Extras linked to nothing yet (music videos imported from disk…): to link.
            let unlinked = catalog.extras.iter().filter(|e| e.links.is_empty()).count();
            if unlinked > 0 || self.showing_unlinked {
                if widgets::chip(ui, &format!("Unlinked extras {}", crate::ui::thousands(unlinked)), self.showing_unlinked) {
                    self.showing_unlinked = !self.showing_unlinked;
                    self.showing_not_imported = false;
                    self.showing_unused = false;
                }
                ui.add_space(8.0);
            }
            // Artists and albums nothing refers to anymore: to merge or leave.
            let unused = unused(catalog).len();
            if unused > 0 || self.showing_unused {
                if widgets::chip(ui, &format!("Nobody uses {}", crate::ui::thousands(unused)), self.showing_unused) {
                    self.showing_unused = !self.showing_unused;
                    self.showing_not_imported = false;
                    self.showing_unlinked = false;
                }
                ui.add_space(8.0);
            }
            if files > 0 || self.showing_not_imported {
                if widgets::chip(ui, &format!("Didn't import {}", crate::ui::thousands(files)), self.showing_not_imported) {
                    self.showing_unlinked = false;
                    self.showing_unused = false;
                    self.showing_not_imported = !self.showing_not_imported;
                }
                ui.add_space(8.0);
            }
            if widgets::chip(ui, "Everything", self.only.is_none() && !self.showing_not_imported && !self.showing_unlinked && !self.showing_unused) {
                self.only = None;
                self.showing_not_imported = false;
                self.showing_unlinked = false;
                self.showing_unused = false;
            }
            for kind in kinds {
                let count = self.counts.get(&kind).copied().unwrap_or(0);
                // Kinds nobody has aren't worth a chip (unless chosen).
                if count == 0 && self.only.as_ref() != Some(&kind) {
                    continue;
                }
                let text = format!("{} {}", vocab.review_kind_name(&kind), crate::ui::thousands(count));
                if widgets::chip(ui, &text, self.only.as_ref() == Some(&kind) && !self.showing_not_imported) {
                    self.only = if self.only.as_ref() == Some(&kind) && !self.showing_not_imported { None } else { Some(kind) };
                    self.showing_not_imported = false;
                    self.showing_unlinked = false;
                    self.showing_unused = false;
                }
            }
        });
        ui.add_space(4.0);
        if self.showing_unlinked {
            ui.add_space(6.0);
            unlinked_extras(ui, shell, catalog, &self.filter);
            return;
        }
        if self.showing_unused {
            ui.add_space(6.0);
            unused_list(ui, shell, catalog, &self.filter);
            return;
        }
        if self.showing_not_imported {
            ui.add_space(6.0);
            self.not_imported.show(ui, shell, catalog, &self.filter);
            return;
        }
        let left = self.entries.len();
        let mut status = format!("{} to go", crate::ui::thousands(left));
        if self.done > 0 {
            status = format!("{} done this session · {status}", self.done);
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(status).color(P.faint).small());
            let tip = if self.only.is_some() { "The bulk tools for what's shown (b)" } else { "Choose a kind of flag first (a chip)" };
            if ui.add_enabled(self.only.is_some() && !self.entries.is_empty(), egui::Button::new("Bulk…").small()).on_hover_text(tip).on_disabled_hover_text(tip).clicked() {
                self.open_bulk(shell, catalog);
            }
        });
        ui.add_space(6.0);
        let entries = &self.entries;
        let id_at = |n: usize| entries.get(n).map(|e| catalog.track_at(e.track).id());
        self.marking.bar(ui, shell, catalog, entries.len(), id_at, "review");

        // ---- The list, and the selected track ----
        let rest = ui.available_rect_before_wrap();
        let list_rect = egui::Rect::from_min_size(rest.min, Vec2::new(LIST_WIDTH, rest.height()));
        let editor_rect = egui::Rect::from_min_max(egui::pos2(list_rect.right() + 24.0, rest.top()), rest.max);
        let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(list_rect));
        self.list(&mut list_ui, shell, catalog, player, &vocab);
        let mut editor_ui = ui.new_child(egui::UiBuilder::new().max_rect(editor_rect));
        // Tracks marked: the change for all of them instead of the one track's form.
        if self.marking.is_empty() {
            self.editor(&mut editor_ui, shell, catalog, player, &vocab);
        } else {
            let entries = &self.entries;
            self.marking.change(&mut editor_ui, shell, entries.len(), |n| entries.get(n).map(|e| catalog.track_at(e.track).id()), None);
        }
        ui.allocate_rect(rest, Sense::hover());
    }

    /// Rebuilds the list when the library, the flags or the filters changed.
    fn build(&mut self, shell: &Shell, catalog: &Catalog) {
        let flags = shell.flags.list.clone();
        let vocab = shell.account.vocab.clone().unwrap_or_default();
        let key = format!("{:?}|{:?}|{}", self.group, self.only, self.filter);
        let built_for = (shell.generation, shell.flags.version, key);
        if self.built_for.as_ref() == Some(&built_for) {
            return;
        }
        // Where each flagged track's file came from, and its import note (from the flags'
        // details: automatic flags don't have them).
        let mut file_of: HashMap<Uuid, &str> = HashMap::new();
        let mut note_of: HashMap<Uuid, &str> = HashMap::new();
        for f in flags.iter() {
            if let Some(path) = f.imported_from.as_deref() {
                file_of.entry(f.track_id).or_insert(path);
            }
            if !f.note().is_empty() {
                note_of.entry(f.track_id).or_insert(f.note());
            }
        }
        // Every track with an open flag (not marked done here), newest flag first.
        let done = &shell.flags.done;
        let mut flagged: Vec<(Uuid, u32, Vec<String>)> = catalog
            .issues
            .iter()
            .filter_map(|(track, open)| {
                let open: Vec<&(Uuid, String)> = open.iter().filter(|(id, _)| !done.contains(id)).collect();
                let newest = open.iter().map(|(id, _)| *id).max()?;
                let mut kinds: Vec<String> = Vec::new();
                for (_, kind) in open {
                    if !kinds.contains(kind) {
                        kinds.push(kind.clone());
                    }
                }
                if self.merged_away.contains(track) {
                    return None;
                }
                Some((newest, catalog.track_index(*track)?, kinds))
            })
            .collect();
        flagged.sort_by_key(|f| std::cmp::Reverse(f.0));
        // Marked tracks that left the list for good aren't marked anymore.
        let still: std::collections::HashSet<u32> = flagged.iter().map(|f| f.1).collect();
        self.marking.retain(|id| catalog.track_index(*id).is_some_and(|i| still.contains(&i)));

        // Words (in the title, artists, album or the file's path) and `key:value` parts.
        let query = crate::catalog::query::Query::parse(&self.filter);
        self.counts.clear();
        let mut entries = Vec::new();
        for (_, i, kinds) in flagged {
            let track = catalog.track_at(i);
            let path = file_of.get(&track.id()).copied().unwrap_or("");
            if !query.matches_track(catalog, i, path) {
                continue;
            }
            for kind in &kinds {
                *self.counts.entry(kind.clone()).or_default() += 1;
            }
            if self.only.as_ref().is_some_and(|only| !kinds.contains(only)) {
                continue;
            }
            let group = match self.group {
                Group::Folder if path.is_empty() => "(folder not known)".to_string(),
                Group::Folder => folder(path).to_string(),
                Group::Note => note_of.get(&track.id()).copied().unwrap_or("(no note)").to_string(),
                Group::Artist if track.artist_display().is_empty() => "(no artist)".to_string(),
                Group::Artist => track.artist_display().to_string(),
                Group::Kind => kinds.first().map(|k| vocab.review_kind_name(k).to_string()).unwrap_or_default(),
                Group::None => String::new(),
            };
            entries.push(Entry { track: i, kinds, group });
        }
        // Groups by name (unknown ones last), the order within kept.
        if self.group != Group::None {
            entries.sort_by(|a, b| (a.group.starts_with('('), &a.group).cmp(&(b.group.starts_with('('), &b.group)));
        }
        let mut rows = Vec::new();
        for (n, e) in entries.iter().enumerate() {
            if self.group != Group::None && (n == 0 || entries[n - 1].group != e.group) {
                let count = entries[n..].iter().take_while(|x| x.group == e.group).count();
                rows.push(Row::Header(e.group.clone(), count));
            }
            rows.push(Row::Track(n));
        }
        self.entries = entries;
        self.rows = rows;
        self.built_for = Some(built_for);

        // The selected track left the list (fixed, marked done): the one now in its place.
        let still_there = self.selected.is_some_and(|id| self.position(catalog, id).is_some());
        if !still_there && !self.entries.is_empty() {
            let at = self.selected_at.min(self.entries.len() - 1);
            self.select(catalog, at);
        }
    }

    fn position(&self, catalog: &Catalog, id: Uuid) -> Option<usize> {
        let index = catalog.track_index(id)?;
        self.entries.iter().position(|e| e.track == index)
    }

    fn select(&mut self, catalog: &Catalog, at: usize) {
        let Some(entry) = self.entries.get(at) else { return };
        self.selected = Some(catalog.track_at(entry.track).id());
        self.selected_at = at;
        self.scroll_to_selected = true;
        self.focus_first = true;
        self.notice = None;
        self.merge = None;
    }

    /// Moves by `by` in the list (and loads the one after, so moving on doesn't wait).
    fn step(&mut self, shell: &mut Shell, catalog: &Catalog, by: isize) {
        if self.entries.is_empty() {
            return;
        }
        let at = self.selected.and_then(|id| self.position(catalog, id)).unwrap_or(self.selected_at);
        let next = (at as isize + by).clamp(0, self.entries.len() as isize - 1) as usize;
        self.select(catalog, next);
        ctx_stop_text(&shell.ctx);
        if let Some(after) = self.entries.get(next + 1) {
            shell.track_details.get(catalog.track_at(after.track).id());
        }
    }

    /// Marks `track`'s open flags done: only "fix later" (`only_fix_later`: a save that
    /// fixed it), else those of the chosen chip's kind, or all of them. If it's the one
    /// selected, on to the next: the list is made again, and if the track is still in it
    /// (it has other flags), the next one is chosen.
    fn mark_done(&mut self, shell: &mut Shell, catalog: &Catalog, track: Uuid, resolution: &str, only_fix_later: bool) {
        let kind: Option<String> = if only_fix_later { Some("fix_later".into()) } else { self.only.clone() };
        let ids: Vec<Uuid> = catalog
            .issues
            .get(&track)
            .into_iter()
            .flatten()
            .filter(|(_, k)| kind.as_ref().is_none_or(|only| k == only))
            .map(|(id, _)| *id)
            .collect();
        if let Some(at) = self.position(catalog, track) {
            self.selected_at = at;
        }
        if !ids.is_empty() {
            shell.flags.resolve(&shell.api, shell.runtime.handle(), &ids, resolution);
            // The library catches up (its flags), for this and every other client.
            shell.act(Action::RefreshLibrary);
            self.done += 1;
            self.build(shell, catalog);
        }
        if self.selected == Some(track) && self.position(catalog, track).is_some() {
            self.step(shell, catalog, 1);
        }
    }

    /// An edit of `track` was applied, leaving it these flags (the server's, checked as
    /// it was written): nothing missing anymore, "fix later" is marked done and the next
    /// track comes; else it says what's left.
    fn after_saved(&mut self, shell: &mut Shell, catalog: &Catalog, track: Uuid, flags: Vec<String>) {
        let vocab = shell.account.vocab.clone().unwrap_or_default();
        let missing: Vec<&String> = flags.iter().filter(|k| vocab.is_automatic(k)).collect();
        if missing.is_empty() {
            // Counted even when it had no "fix later" (only something missing).
            let fix_later = catalog.issues.get(&track).is_some_and(|f| f.iter().any(|(_, k)| k == "fix_later"));
            if !fix_later {
                self.done += 1;
            }
            self.mark_done(shell, catalog, track, "fixed", true);
        } else {
            let names: Vec<&str> = missing.iter().map(|k| vocab.review_kind_name(k)).collect();
            self.notice = Some(format!("Saved. Still: {}.", names.join(", ")));
            self.fresh = Some((track, missing.into_iter().cloned().collect(), shell.generation));
            self.focus_first = true;
        }
    }

    /// The tracks a duplicate flag names that are still there to compare: not merged
    /// away here, not the flagged track itself, and in the library.
    fn duplicates(&self, flag: &Flag, catalog: &Catalog) -> Vec<Uuid> {
        duplicate_ids(flag)
            .into_iter()
            .filter(|id| *id != flag.track_id && !self.merged_away.contains(id) && catalog.track_index(*id).is_some())
            .collect()
    }

    /// Opens the merge view with the first duplicate of the selected track not judged
    /// yet: this one on the left (kept), the other on the right (merged into it; `s`
    /// swaps). Kept, so a track with several duplicates stays in the list and the next
    /// one can be merged into it too.
    fn open_merge(&mut self, shell: &Shell, catalog: &Catalog) {
        let Some(id) = self.selected else { return };
        let other = shell
            .flags
            .list
            .iter()
            .filter(|f| f.track_id == id && f.kind == "possible_duplicate")
            .flat_map(|f| self.duplicates(f, catalog))
            .find(|other| !self.not_same.contains(&(id, *other)));
        match other {
            Some(other) => self.merge = Some(MergeView::new(id, other)),
            None => self.notice = Some("No possible duplicate to compare here.".into()),
        }
    }

    /// `track` and `other` aren't the same song: a duplicate flag on `track` is settled
    /// once every track it names has been judged so.
    fn judged_not_same(&mut self, shell: &mut Shell, catalog: &Catalog, track: Uuid, other: Uuid) {
        self.not_same.insert((track, other));
        let settled: Vec<Uuid> = shell
            .flags
            .list
            .iter()
            .filter(|f| f.track_id == track && f.kind == "possible_duplicate")
            .filter(|f| self.duplicates(f, catalog).iter().all(|o| self.not_same.contains(&(track, *o))))
            .map(|f| f.id)
            .collect();
        if settled.is_empty() {
            self.notice = Some("Noted: not the same. It names other tracks too; judge those as well.".into());
        } else {
            shell.flags.resolve(&shell.api, shell.runtime.handle(), &settled, "not a duplicate");
            self.done += 1;
            self.notice = Some("Marked: not the same.".into());
        }
    }

    /// The bulk tools, for the chosen chip's kind and the tracks shown.
    fn open_bulk(&mut self, shell: &Shell, catalog: &Catalog) {
        let Some(kind) = self.only.clone() else { return };
        let tracks: Vec<u32> = self.entries.iter().map(|e| e.track).collect();
        self.bulk = Some(super::review_bulk::ReviewBulk::open(shell, catalog, &kind, tracks, &self.filter));
    }

    fn keys(&mut self, ctx: &egui::Context, shell: &mut Shell, catalog: &Catalog, player: &PlayerView) {
        if shell.palette_open || shell.modal_open {
            return;
        }
        // The marks' keys first (Ctrl+M is theirs while tracks are marked).
        let at = self.selected.and_then(|id| self.position(catalog, id)).unwrap_or(self.selected_at);
        let entries = &self.entries;
        let id_at = |n: usize| entries.get(n).map(|e| catalog.track_at(e.track).id());
        self.marking.keys(ctx, shell, at, entries.len(), id_at);
        let (next, previous, merge) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::ArrowDown),
                i.consume_key(Modifiers::COMMAND, Key::ArrowUp),
                // Ctrl+M: works in a text field too (and plain m is mute).
                i.consume_key(Modifiers::COMMAND, Key::M),
            )
        });
        if merge {
            if self.merge.is_some() {
                self.merge = None;
            } else {
                self.open_merge(shell, catalog);
            }
        }
        if next {
            self.step(shell, catalog, 1);
        }
        if previous {
            self.step(shell, catalog, -1);
        }
        // The merge view, while open, has its own keys (Esc closes it, not the page).
        if self.merge.is_some() {
            shell.form_dirty = true;
            return;
        }
        // Ctrl+Enter with nothing to change: it was checked and is fine as it is. (With
        // changes, it's the editor's: it applies them. With tracks marked, the change for
        // them is shown instead.)
        let editor = self.selected.and_then(|id| shell.track_editors.get(&id));
        let blank = editor.is_none_or(|e| e.is_blank() && !e.applying_or_applied());
        let rows = editor.is_some_and(|e| e.takes_keys());
        if self.marking.is_empty()
            && blank
            && let Some(id) = self.selected
            && ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter))
        {
            self.mark_done(shell, catalog, id, "checked: fine as it is", true);
        }
        // Ctrl+D: done, in a text field too (plain d is typed there).
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::D))
            && self.marking.is_empty()
            && let Some(id) = self.selected
        {
            self.mark_done(shell, catalog, id, "done", false);
            return;
        }
        // In a text field, or on the editor's rows: the keys are theirs.
        if ctx.egui_wants_keyboard_input() || rows {
            return;
        }
        let key = |k: Key| ctx.input(|i| i.key_pressed(k) && i.modifiers.is_none());
        let shift = |k: Key| ctx.input(|i| i.key_pressed(k) && i.modifiers.matches_exact(Modifiers::SHIFT));
        let mut acted = true;
        if shift(Key::ArrowDown) || shift(Key::ArrowUp) {
            // A range: the rows passed over take the starting row's state.
            self.step(shell, catalog, if shift(Key::ArrowDown) { 1 } else { -1 });
            // (The cursor stays out of the form: the next Shift+↓/↑ goes on.)
            self.focus_first = false;
            let to = self.selected.and_then(|id| self.position(catalog, id)).unwrap_or(at);
            let entries = &self.entries;
            self.marking.range_to(at, to, |n| entries.get(n).map(|e| catalog.track_at(e.track).id()));
        } else if key(Key::J) || key(Key::ArrowDown) {
            self.step(shell, catalog, 1);
        } else if key(Key::K) || key(Key::ArrowUp) {
            self.step(shell, catalog, -1);
        } else if key(Key::U) {
            self.step(shell, catalog, 1);
        } else if key(Key::D)
            && self.marking.is_empty()
            && let Some(id) = self.selected
        {
            self.mark_done(shell, catalog, id, "done", false);
        } else if key(Key::B) && self.only.is_some() && !self.entries.is_empty() {
            self.open_bulk(shell, catalog);
        } else if key(Key::Space)
            && let Some(id) = self.selected
        {
            if playing_track(player) == Some(id) {
                shell.act(Action::TogglePause);
            } else if let Some(index) = catalog.track_index(id) {
                shell.act(Action::Play { tracks: vec![index], start: 0 });
            }
        } else {
            acted = false;
        }
        // The key's character is still in this frame's input: taken out, or the next
        // track's first field (which gets the cursor right away) would type it ("d").
        if acted {
            ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
        }
    }

    fn list(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog, player: &PlayerView, vocab: &Vocab) {
        if self.rows.is_empty() {
            let text = if shell.flags.list.is_empty() && shell.flags.loading {
                "Loading the flags…".to_string()
            } else {
                match &self.only {
                    Some(kind) => format!("Nothing with “{}” (or nothing matches the filter).", vocab.review_kind_name(kind)),
                    None => "Nothing needs a look (or nothing matches the filter).".to_string(),
                }
            };
            ui.label(RichText::new(text).color(P.muted));
            return;
        }
        let selected_row = self.selected.and_then(|id| self.position(catalog, id)).and_then(|e| self.rows.iter().position(|r| matches!(r, Row::Track(n) if *n == e)));
        let mut area = egui::ScrollArea::vertical().id_salt("review-list").auto_shrink([false, false]);
        if self.scroll_to_selected {
            self.scroll_to_selected = false;
            if let Some(row) = selected_row {
                area = area.vertical_scroll_offset((row as f32 * ROW - ui.available_height() / 2.0).max(0.0));
            }
        }
        let playing = playing_track(player);
        let (mut clicked, mut boxed) = (None, None);
        // No gap between rows, set before `show_rows` so its row pitch is exactly ROW (the scroll offsets above rely on it).
        ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        area.show_rows(ui, ROW, self.rows.len(), |ui, range| {
            for r in range {
                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
                let painter = ui.painter();
                match &self.rows[r] {
                    Row::Header(name, count) => {
                        let text = format!("{name}  ·  {count}");
                        widgets::text_in(painter, egui::Rect::from_min_size(rect.min + Vec2::new(6.0, 22.0), Vec2::new(rect.width() - 12.0, 16.0)), &text, font(11.5, semibold()), P.faint);
                    }
                    Row::Track(n) => {
                        let e = &self.entries[*n];
                        let t = catalog.track_at(e.track);
                        if Some(r) == selected_row {
                            painter.rect_filled(rect, 8, P.surface_hi);
                        } else if response.hovered() {
                            painter.rect_filled(rect, 8, P.surface);
                        }
                        // The mark's box, in front.
                        let mark = egui::Rect::from_min_size(rect.min + Vec2::new(8.0, (ROW - 18.0) / 2.0), Vec2::splat(18.0));
                        let on_box = |pos: Option<egui::Pos2>| pos.is_some_and(|p| mark.expand(4.0).contains(p));
                        super::marking::paint_box(painter, mark, self.marking.contains(&t.id()), on_box(response.hover_pos()));
                        let text_left = mark.right() + 10.0 - rect.left();
                        let title = if t.title().trim().is_empty() { "(no title)" } else { t.title() };
                        let color = if playing == Some(t.id()) { P.accent } else { P.text };
                        let unsaved = shell.track_editors.get(&t.id()).is_some_and(|e| e.has_changes());
                        let title = if unsaved { format!("● {title}") } else { title.to_string() };
                        widgets::text_in_og(painter, egui::Rect::from_min_size(rect.min + Vec2::new(text_left, 6.0), Vec2::new(rect.width() - text_left - 10.0, 18.0)), &title, font(13.5, medium()), color, t.original_filename().is_some());
                        let mut sub: Vec<String> = Vec::new();
                        if !t.artist_display().is_empty() {
                            sub.push(t.artist_display().to_string());
                        }
                        // Its flags, but not the chip's own kind (every row has that).
                        let issues: Vec<String> =
                            e.kinds.iter().filter(|k| Some(*k) != self.only.as_ref()).map(|k| vocab.review_kind_name(k).to_lowercase()).collect();
                        if !issues.is_empty() {
                            sub.push(issues.join(", "));
                        }
                        widgets::text_in(painter, egui::Rect::from_min_size(rect.min + Vec2::new(text_left, 25.0), Vec2::new(rect.width() - text_left - 10.0, 16.0)), &sub.join("  ·  "), font(12.0, medium()), P.muted);
                        if response.clicked() {
                            if on_box(response.interact_pointer_pos()) {
                                boxed = Some(*n);
                            } else {
                                clicked = Some(*n);
                            }
                        }
                    }
                }
            }
        });
        });
        // Marking leaves the form as it is; a plain click opens the track.
        let modifiers = ui.input(|i| i.modifiers);
        let from = self.selected.and_then(|id| self.position(catalog, id)).unwrap_or(self.selected_at);
        let entries = &self.entries;
        let id_at = |n: usize| entries.get(n).map(|e| catalog.track_at(e.track).id());
        if let Some(n) = boxed {
            self.marking.toggle(n, id_at);
        } else if let Some(n) = clicked {
            self.marking.click(n, from, modifiers, id_at);
            if !modifiers.command && !modifiers.shift {
                self.select(catalog, n);
                self.scroll_to_selected = false;
            }
        }
    }

    fn editor(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, player: &PlayerView, vocab: &Vocab) {
        let Some(id) = self.selected else { return };
        let Some(index) = catalog.track_index(id) else { return };
        let track = catalog.track_at(index);
        let flags: Vec<Flag> = shell.flags.list.iter().filter(|f| f.track_id == id).cloned().collect();
        let path = flags.iter().find_map(|f| f.imported_from.clone());
        let cached = shell.track_details.get(id);
        let (detail, error) = (cached.value.clone(), cached.error.clone());
        // Taken out of the shell while it's shown (showing it needs the shell too).
        let mut ed = shell.track_editors.remove(&id).unwrap_or_default();
        let editor = &mut ed;

        egui::ScrollArea::vertical().id_salt(("review-editor", id)).auto_shrink([false, false]).show(ui, |ui| {
            // ---- What it is, and what's wrong ----
            ui.horizontal(|ui| {
                let size = Vec2::splat(64.0);
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                shell.cover(ui, track.cover(), id, rect, 6, 0.2);
                ui.vertical(|ui| {
                    let title = if track.title().trim().is_empty() { "(no title)" } else { track.title() };
                    ui.label(RichText::new(title).font(font(22.0, bold())));
                    ui.label(RichText::new(if track.artist_display().is_empty() { "(no artists)" } else { track.artist_display() }).color(P.muted));
                    let playing = playing_track(player) == Some(id) && !player.paused;
                    let text = if playing { "Pause  (Space)" } else { "Play  (Space)" };
                    if ui.small_button(text).clicked() {
                        if playing_track(player) == Some(id) {
                            shell.act(Action::TogglePause);
                        } else {
                            shell.act(Action::Play { tracks: vec![index], start: 0 });
                        }
                    }
                });
            });
            ui.add_space(6.0);
            if let Some(path) = &path {
                ui.label(RichText::new(format!("File: {}", file_name(path))).color(P.text));
                ui.label(RichText::new(format!("in {}", folder(path))).color(P.faint).small());
            }
            for f in &flags {
                let mut line = vocab.review_kind_name(&f.kind).to_string();
                if !f.note().is_empty() {
                    line = format!("{line}: {}", f.note());
                }
                if let Some(reason) = f.related.get("reason").and_then(|r| r.as_str()) {
                    line = format!("{line} ({reason})");
                }
                ui.label(RichText::new(line).color(P.muted));
                related(ui, shell, f);
                if f.kind == "ambiguous_artist" {
                    let name = f.related.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                    ui.label(RichText::new(format!("Which artist “{name}” is: on its credit below (click one, or c on the row).")).color(P.faint).small());
                }
                if f.kind == "possible_duplicate" {
                    let others = self.duplicates(f, catalog);
                    if others.is_empty() {
                        ui.label(RichText::new("Nothing left to compare (merged already): d marks it done.").color(P.faint).small());
                    }
                    for other in others {
                        let judged = self.not_same.contains(&(id, other));
                        let text = if judged { "judged not the same" } else { "Compare and merge…  (Ctrl+M)" };
                        if ui.add_enabled(!judged, egui::Button::new(text).small()).clicked() {
                            self.merge = Some(MergeView::new(id, other));
                        }
                    }
                }
            }

            // Two tracks side by side instead of the form.
            if let Some(view) = self.merge.as_mut() {
                ui.add_space(10.0);
                let view_pair = (view.left, view.right);
                match view.show(ui, shell, catalog, player) {
                    Some(Outcome::Merged { kept, gone }) => {
                        self.merge = None;
                        self.merged_away.insert(gone);
                        self.done += 1;
                        self.notice = Some(if kept == id {
                            "Merged into this track. Undo is on its page.".into()
                        } else {
                            "Merged. Undo is on the kept track's page.".into()
                        });
                        shell.track_details.refresh(kept);
                        shell.track_editors.remove(&gone);
                        shell.flags.refresh(&shell.api, shell.runtime.handle(), &shell.waker);
                        shell.act(Action::RefreshLibrary);
                    }
                    Some(Outcome::NotSame) => {
                        let pair = (view.left, view.right);
                        self.merge = None;
                        self.judged_not_same(shell, catalog, id, if pair.0 == id { pair.1 } else { pair.0 });
                    }
                    Some(Outcome::Shared) => {
                        self.merge = None;
                        self.done += 1;
                        self.notice = Some("Both kept, sharing one file. The other file stays as a previous file.".into());
                        shell.track_details.refresh(view_pair.0);
                        shell.track_details.refresh(view_pair.1);
                        shell.flags.refresh(&shell.api, shell.runtime.handle(), &shell.waker);
                        shell.act(Action::RefreshLibrary);
                    }
                    Some(Outcome::Closed) => self.merge = None,
                    None => shell.form_dirty = true,
                }
                return;
            }

            let Some(d) = detail else {
                let text = error.map(|e| format!("Couldn't load the track: {e}")).unwrap_or_else(|| "Loading…".into());
                ui.label(RichText::new(text).color(P.faint));
                return;
            };

            // What it's missing, as the server says (its open automatic flags): as the edit
            // just applied left it, until the library has caught up.
            let missing: Vec<String> = match &self.fresh {
                Some((track, fresh, generation)) if *track == id && *generation == shell.generation => fresh.clone(),
                _ => catalog
                    .issues
                    .get(&id)
                    .into_iter()
                    .flatten()
                    .filter(|(flag, kind)| vocab.is_automatic(kind) && !shell.flags.done.contains(flag))
                    .map(|(_, kind)| kind.clone())
                    .collect(),
            };
            ui.add_space(6.0);
            if missing.is_empty() {
                ui.label(RichText::new("Nothing missing. Ctrl+Enter: done as it is (or apply what's staged, and go on).").color(P.accent));
            } else {
                let names: Vec<&str> = missing.iter().map(|k| vocab.review_kind_name(k)).collect();
                ui.label(RichText::new(names.join(" · ")).font(font(15.0, semibold())).color(P.danger));
            }
            if let Some(notice) = &self.notice {
                ui.label(RichText::new(notice).color(P.muted));
            }
            ui.label(RichText::new("Ctrl+Enter apply and next · Ctrl+↓/↑ next/previous · Esc leaves the commandline; then j/k, Space play, u skip, d done").color(P.faint).small());
            ui.add_space(8.0);

            // A line prefilled for a flag that's gone since (fixed by a bulk edit) goes too;
            // only that one, not the same text typed by hand. (Typing exactly the prefill
            // in the very frame its flag goes would lose it too: harmless, nobody wants
            // "source:" once the source is fixed.)
            if let Some((track, kind, line)) = &self.prefilled
                && *track == id
                && !missing.iter().any(|k| k == kind)
            {
                editor.clear_line_if(line);
                self.prefilled = None;
            }

            // The line that fixes the first thing missing (once, when the track is shown),
            // once the editor has the track as it is (its title, not the library's older one).
            if self.focus_first
                && let Some(title) = editor.title(id)
            {
                self.focus_first = false;
                let line = missing.iter().find_map(|k| crate::ui::edit::editor::line_for(k, &title).map(|l| (k.clone(), l)));
                let line = line.map(|(kind, l)| {
                    self.prefilled = Some((id, kind, l.clone()));
                    l
                });
                match line {
                    Some(line) if editor.is_blank() => editor.prefill(&line),
                    _ => editor.opened(),
                }
                // (Shown, not asked for: a search being typed keeps the cursor.)
                editor.gently();
            }

            editor.show(ui, shell, catalog, &[id], None);
            if let Some(flags_after) = editor.take_saved() {
                self.after_saved(shell, catalog, id, flags_after);
            }


            // The file's own tags, to fix from: a click puts one in its field.
            let tags = d.file.embedded_tags.clone();
            if let Some(tags) = tags.as_object().filter(|t| !t.is_empty()) {
                ui.add_space(12.0);
                ui.label(RichText::new("Tags in the file").font(font(14.0, bold())));
                ui.label(RichText::new("Click one to use it: it goes in the editor's commandline (title:…, artist:…, album:…, date:…).").color(P.faint).small());
                egui::Grid::new(("review-tags", id)).num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                    for (name, value) in tags {
                        let text = match value {
                            serde_json::Value::String(s) => s.clone(),
                            serde_json::Value::Array(a) => a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("; "),
                            other => other.to_string(),
                        };
                        ui.label(RichText::new(name).color(P.faint));
                        let field = tag_field(name);
                        let label = egui::Label::new(RichText::new(&text).color(if field.is_some() { P.text } else { P.muted })).sense(Sense::click()).truncate();
                        let response = ui.add(label);
                        if let Some(field) = field
                            && response.on_hover_text(format!("Use as the {field}")).clicked()
                        {
                            let key = match field {
                                "release date" => "date",
                                other => other,
                            };
                            editor.prefill(&format!("{key}:{}", crate::ui::edit::editor::quoted(&text)));
                        }
                        ui.end_row();
                    }
                });
            }
        });
        // (Not back if it was merged away meanwhile.)
        if !self.merged_away.contains(&id) {
            shell.track_editors.insert(id, ed);
        }
    }
}

/// The field a tag fills, by its name (as the file has it, in any case).
fn tag_field(name: &str) -> Option<&'static str> {
    match name.to_lowercase().as_str() {
        "title" | "tit2" => Some("title"),
        "artist" | "tpe1" | "albumartist" | "album_artist" | "tpe2" => Some("artist"),
        "album" | "talb" => Some("album"),
        "date" | "year" | "tdrc" | "tyer" | "originaldate" => Some("release date"),
        _ => None,
    }
}

/// Something nobody uses: an artist or an album.
enum Unused<'a> {
    Artist(&'a crate::catalog::model::Artist),
    Album(crate::catalog::Album<'a>),
}

/// Artists credited on nothing (no track, no album, in no grouping) and albums without
/// tracks, as far as the user sees: left behind when credits or tracks moved ("A feat. B"
/// after its credits were fixed). They can't be deleted (old revisions refer to them);
/// an artist is usually merged into the right one.
fn unused(catalog: &Catalog) -> Vec<Unused<'_>> {
    let members: std::collections::HashSet<uuid::Uuid> = catalog.groupings.iter().flat_map(|g| g.artists.iter().copied()).collect();
    let artists = catalog.artists.iter().filter(|a| a.track_count == 0 && a.album_count == 0 && !members.contains(&a.id)).map(Unused::Artist);
    let albums = catalog.albums().filter(|a| a.track_count() == 0).map(Unused::Album);
    artists.chain(albums).collect()
}

/// `unused`, by name, each opening its page (where an artist is merged). `filter`: words
/// in the name.
fn unused_list(ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog, filter: &str) {
    let words: Vec<String> = filter.to_lowercase().split_whitespace().map(str::to_string).collect();
    let name = |u: &Unused| match u {
        Unused::Artist(a) => a.label(),
        Unused::Album(a) => a.title().to_string(),
    };
    let mut shown: Vec<Unused> = unused(catalog).into_iter().filter(|u| {
        let text = name(u).to_lowercase();
        words.iter().all(|w| text.contains(w.as_str()))
    }).collect();
    shown.sort_by_key(|u| name(u).to_lowercase());
    ui.label(RichText::new("Artists credited on nothing and albums without tracks (as far as you can see). They can't be deleted, old versions refer to them: open one to merge it into the right one, or leave it.").color(P.muted));
    ui.add_space(6.0);
    if shown.is_empty() {
        ui.label(RichText::new("None (or nothing matches the filter).").color(P.faint));
        return;
    }
    egui::ScrollArea::vertical().id_salt("unused").auto_shrink([false, false]).show_rows(ui, 32.0, shown.len(), |ui, range| {
        for u in &shown[range] {
            let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width().min(760.0), 32.0), Sense::click());
            if response.hovered() {
                ui.painter().rect_filled(rect, 6, P.surface);
            }
            let (what, page) = match u {
                Unused::Artist(a) => ("artist", crate::ui::Page::Artist(a.id)),
                Unused::Album(a) => ("album", crate::ui::Page::Album(a.id())),
            };
            ui.painter().text(egui::pos2(rect.left() + 10.0, rect.center().y), egui::Align2::LEFT_CENTER, name(u), font(14.0, semibold()), P.text);
            ui.painter().text(egui::pos2(rect.right() - 10.0, rect.center().y), egui::Align2::RIGHT_CENTER, what, font(12.0, medium()), P.faint);
            if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                shell.act(Action::Go(page));
            }
        }
    });
}

/// Extras linked to nothing (music videos imported from disk…), by name, to link from their
/// pages. `filter`: words in the name or kind.
fn unlinked_extras(ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog, filter: &str) {
    let words: Vec<String> = filter.to_lowercase().split_whitespace().map(str::to_string).collect();
    let mut shown: Vec<&crate::catalog::model::Extra> = catalog
        .extras
        .iter()
        .filter(|e| e.links.is_empty())
        .filter(|e| {
            let text = format!("{} {}", e.name, e.kind).to_lowercase();
            words.iter().all(|w| text.contains(w.as_str()))
        })
        .collect();
    shown.sort_by_key(|e| e.name.to_lowercase());
    ui.label(RichText::new("Linked to nothing yet: open one to link it to its song (“Link to a track…”) or its artist.").color(P.muted));
    ui.add_space(6.0);
    if shown.is_empty() {
        ui.label(RichText::new("None (or nothing matches the filter).").color(P.faint));
        return;
    }
    egui::ScrollArea::vertical().id_salt("unlinked-extras").auto_shrink([false, false]).show_rows(ui, 52.0, shown.len(), |ui, range| {
        for e in &shown[range] {
            let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width().min(760.0), 52.0), Sense::click());
            if response.hovered() {
                ui.painter().rect_filled(rect, 8, P.surface);
            }
            let thumb = egui::Rect::from_min_size(rect.min + Vec2::new(6.0, 6.0), Vec2::new(71.0, 40.0));
            match e.thumb() {
                Some(cover) => shell.cover(ui, Some(cover), e.id, thumb, 4, 0.0),
                None => {
                    ui.painter().rect_stroke(thumb, 4, egui::Stroke::new(1.0, P.line), egui::StrokeKind::Inside);
                }
            }
            let left = thumb.right() + 12.0;
            ui.painter().text(egui::pos2(left, rect.top() + 9.0), egui::Align2::LEFT_TOP, &e.name, font(14.0, semibold()), P.text);
            let files = crate::ui::edit::extras::files_text(e.files as usize);
            ui.painter().text(egui::pos2(left, rect.top() + 29.0), egui::Align2::LEFT_TOP, format!("{} · {files}", e.kind), font(12.0, medium()), P.muted);
            if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                shell.act(Action::Go(crate::ui::Page::Extra(e.id)));
            }
        }
    });
}

/// The tracks a duplicate flag names.
fn duplicate_ids(flag: &Flag) -> Vec<Uuid> {
    let tracks = flag.related_tracks.as_array().cloned().unwrap_or_default();
    tracks.iter().filter_map(|t| t.get("id")?.as_str()?.parse().ok()).collect()
}

/// A flag's other tracks or artists, as links.
fn related(ui: &mut egui::Ui, shell: &mut Shell, flag: &Flag) {
    let tracks = flag.related_tracks.as_array().cloned().unwrap_or_default();
    let artists = flag.related_artists.as_array().cloned().unwrap_or_default();
    if tracks.is_empty() && artists.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for t in &tracks {
            let (Some(id), Some(title)) = (t.get("id").and_then(|v| v.as_str()).and_then(|v| v.parse::<Uuid>().ok()), t.get("title").and_then(|v| v.as_str())) else {
                continue;
            };
            let artist = t.get("artist_display").and_then(|v| v.as_str()).unwrap_or("");
            if widgets::link(ui, &format!("{title} · {artist}"), font(13.0, medium()), P.text) {
                shell.act(Action::Go(crate::ui::Page::Track(id)));
            }
        }
        for a in &artists {
            let (Some(id), Some(name)) = (a.get("id").and_then(|v| v.as_str()).and_then(|v| v.parse::<Uuid>().ok()), a.get("name").and_then(|v| v.as_str())) else {
                continue;
            };
            let dis = a.get("disambiguation").and_then(|v| v.as_str()).filter(|d| !d.is_empty()).map(|d| format!(" ({d})")).unwrap_or_default();
            let label = shell.catalog.value.as_ref().and_then(|c| c.artist_label(id)).unwrap_or_else(|| format!("{name}{dis}"));
            if widgets::link(ui, &label, font(13.0, medium()), P.text) {
                shell.act(Action::Go(crate::ui::Page::Artist(id)));
            }
        }
    });
}

/// Leaves the text field (moving to another track: the cursor goes to that one's first
/// field instead).
fn ctx_stop_text(ctx: &egui::Context) {
    ctx.memory_mut(|m| m.stop_text_input());
}

/// The file name in a path ("/music/a/Song.mp3" → "Song.mp3").
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The folder of a path ("/music/a/Song.mp3" → "/music/a").
fn folder(path: &str) -> &str {
    path.rfind(['/', '\\']).map_or("", |i| &path[..i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(file_name("/music/a/Song.mp3"), "Song.mp3");
        assert_eq!(folder("/music/a/Song.mp3"), "/music/a");
        assert_eq!(folder("Song.mp3"), "");
    }
}
