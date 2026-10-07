//! "Credited as…" for the marked tracks (edit mode): an artist credited on them (all its
//! credits, or only those under one of its names now), credited under another of its
//! names, or to another artist altogether (a name that was really someone else's), on
//! all of them and on the albums they're on at once. A row per artist credited on the
//! marked tracks says how it's credited now; each choice asks the server what would
//! change (the exact tracks and albums, nothing kept), then the button does it (a
//! revision each). See the server's `db::credited_as`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText, Vec2};
use serde::Deserialize;
use uuid::Uuid;

use crate::catalog::search::Part;
use crate::catalog::Catalog;
use crate::ui::edit::inline_search::{Choice as Choice2, InlineSearch};
use crate::ui::theme::{font, semibold, PALETTE as P};
use crate::ui::{thousands, widgets, Action, Shell};

#[derive(Debug, Clone, Deserialize)]
struct Named {
    id: Uuid,
    title: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Answer {
    tracks: Vec<Named>,
    albums: Vec<Named>,
    already: usize,
    not_credited: usize,
    not_found: usize,
}

/// An artist credited on the marked tracks: on how many, and under which names now
/// (None: its own), with how many each.
struct Row {
    artist: Uuid,
    name: String,
    tracks: usize,
    now: Vec<(Option<String>, usize)>,
}

/// Which of the artist's credits: all, those under its own name, or those under an alias
/// (by id).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Only {
    All,
    OwnName,
    Alias(Uuid),
}

/// What's asked about: the artist, which of its credits, and the artist and name they
/// become (alias None: that artist's own name).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Choice {
    from: Uuid,
    only: Only,
    to: Uuid,
    alias: Option<Uuid>,
}

pub struct CreditedAs {
    tracks: Vec<Uuid>,
    rows: Vec<Row>,
    /// The library the rows were counted from.
    counted_for: u64,
    artist: Option<Uuid>,
    only: Only,
    /// Credited to another artist (its id and name) instead; None: the same one.
    to: Option<(Uuid, String)>,
    search: InlineSearch,
    alias: Option<Option<Uuid>>,
    reason: String,
    /// The preview's answer, with the choice it's for (one for another is dropped).
    preview: Arc<Mutex<Option<(Choice, Result<Answer, String>)>>>,
    loaded: Option<Result<Answer, String>>,
    sending: bool,
    done: Arc<Mutex<Option<Result<Answer, String>>>>,
    /// What the last one did (or why not).
    outcome: Option<Result<String, String>>,
}

impl CreditedAs {
    pub fn open(tracks: Vec<Uuid>) -> CreditedAs {
        CreditedAs {
            tracks,
            rows: Vec::new(),
            counted_for: u64::MAX,
            artist: None,
            only: Only::All,
            to: None,
            search: InlineSearch::new(Part::Artists),
            alias: None,
            reason: String::new(),
            preview: Arc::default(),
            loaded: None,
            sending: false,
            done: Arc::default(),
            outcome: None,
        }
    }

    fn choice(&self) -> Option<Choice> {
        let from = self.artist?;
        let to = self.to.as_ref().map(|t| t.0).unwrap_or(from);
        Some(Choice { from, only: self.only, to, alias: self.alias? })
    }

    /// The popup. True when it's closed.
    pub fn show(&mut self, shell: &mut Shell) -> bool {
        let Some(catalog) = shell.catalog.value.clone() else { return true };
        if self.counted_for != shell.generation {
            self.counted_for = shell.generation;
            self.rows = rows(&catalog, &self.tracks);
        }
        let choice = self.choice();
        let previewed = self.preview.lock().unwrap().take();
        if let Some((asked, result)) = previewed
            && Some(asked) == choice
        {
            self.loaded = Some(result);
        }
        let finished = self.done.lock().unwrap().take();
        if let Some(result) = finished {
            self.sending = false;
            shell.act(Action::RefreshLibrary);
            self.outcome = Some(result.map(|a| format!("Done: {} tracks and {} albums credited anew.", thousands(a.tracks.len()), thousands(a.albums.len()))));
            // Asked again: what's left to change is nothing now.
            self.ask(shell);
        }
        // The names of the chosen artist, and of the one credited instead (their aliases,
        // from their details).
        let mut names = |artist: Option<Uuid>| -> Vec<(Uuid, String)> {
            match artist {
                Some(artist) => shell.artist_details.get(artist).value.as_ref().map(|d| d.aliases.iter().map(|a| (a.id, a.name.clone())).collect()).unwrap_or_default(),
                None => Vec::new(),
            }
        };
        let from_names = names(self.artist);
        let to_names = names(self.to.as_ref().map(|t| t.0).or(self.artist));
        shell.modal_open = true;
        let ctx = shell.ctx.clone();
        let title = format!("Credited as… on {} tracks", thousands(self.tracks.len()));
        let mut pick_artist = None;
        let mut pick_only = None;
        let mut pick_to: Option<Option<(Uuid, String)>> = None;
        let mut pick_alias = None;
        let mut go = false;
        let mut cancel = false;
        let mut open = None;
        let closed = crate::ui::modal::show(&ctx, "credited-as", &title, Vec2::new(900.0, 780.0), |ui| {
            ui.label("An artist's credits on these tracks, and on the albums they're on: under another of its names, or credited to another artist (a name that was really someone else's). A revision each that changes; other artists' credits stay as they are.");
            ui.add_space(8.0);
            ui.label(RichText::new("Which artist").font(font(14.0, semibold())));
            egui::ScrollArea::vertical().id_salt("credited-as-artists").max_height(160.0).show(ui, |ui| {
                for row in &self.rows {
                    ui.horizontal_wrapped(|ui| {
                        if widgets::chip(ui, &format!("{}  {}", row.name, thousands(row.tracks)), self.artist == Some(row.artist)) {
                            pick_artist = Some(row.artist);
                        }
                        let now: Vec<String> = row.now.iter().map(|(name, n)| format!("{} {}", name.as_deref().map(|a| format!("as “{a}”")).unwrap_or_else(|| "own name".into()), thousands(*n))).collect();
                        ui.label(RichText::new(now.join(" · ")).color(P.muted));
                    });
                }
            });
            let Some(artist) = self.artist else {
                return;
            };
            let Some(row) = self.rows.iter().find(|r| r.artist == artist) else { return };
            let name = row.name.clone();

            // Which of its credits: all, or those under one of its names now.
            ui.add_space(8.0);
            ui.label(RichText::new("Which credits").font(font(14.0, semibold())));
            ui.horizontal_wrapped(|ui| {
                if widgets::chip(ui, &format!("all {}", thousands(row.tracks)), self.only == Only::All) {
                    pick_only = Some(Only::All);
                }
                for (now, n) in &row.now {
                    match now {
                        None => {
                            if widgets::chip(ui, &format!("under its own name {}", thousands(*n)), self.only == Only::OwnName) {
                                pick_only = Some(Only::OwnName);
                            }
                        }
                        Some(alias) => match from_names.iter().find(|(_, a)| a == alias) {
                            Some((id, _)) => {
                                if widgets::chip(ui, &format!("as “{alias}” {}", thousands(*n)), self.only == Only::Alias(*id)) {
                                    pick_only = Some(Only::Alias(*id));
                                }
                            }
                            // A hidden name (not listed on the artist): can't be told apart here.
                            None => {
                                ui.add_enabled(false, egui::Button::new(format!("as “{alias}” {}", thousands(*n)))).on_disabled_hover_text("Not one of its listed names (hidden, or its details are still loading)");
                            }
                        },
                    }
                }
            });

            // Credited to: the same artist, or another.
            ui.add_space(8.0);
            ui.label(RichText::new("Credit to").font(font(14.0, semibold())));
            ui.horizontal_wrapped(|ui| {
                if widgets::chip(ui, &format!("{name} (the same artist)"), self.to.is_none()) {
                    pick_to = Some(None);
                }
                if let Some((_, other)) = &self.to {
                    widgets::chip(ui, other, true);
                }
            });
            let hit = self.search.show(ui, shell, Some(&catalog), egui::Id::new("credited-as-to"), "another artist: type its name", &[]);
            if let Some((Choice2::Hit(i), _)) = hit
                && let Some(a) = catalog.artists.get(i as usize)
            {
                pick_to = Some((a.id != artist).then(|| (a.id, a.name.clone())));
            }
            let to_name = self.to.as_ref().map(|t| t.1.clone()).unwrap_or_else(|| name.clone());

            ui.add_space(8.0);
            ui.label(RichText::new("As").font(font(14.0, semibold())));
            ui.horizontal_wrapped(|ui| {
                if widgets::chip(ui, &format!("{to_name} (its own name)"), self.alias == Some(None)) {
                    pick_alias = Some(None);
                }
                for (id, alias) in &to_names {
                    if widgets::chip(ui, alias, self.alias == Some(Some(*id))) {
                        pick_alias = Some(Some(*id));
                    }
                }
                if to_names.is_empty() {
                    ui.label(RichText::new("No other names: add one on its page (edit mode).").color(P.faint));
                }
            });
            if self.alias.is_none() {
                return;
            }
            ui.add_space(8.0);
            let answer = match &self.loaded {
                None => {
                    ui.label(RichText::new("Asking the server what would change…").color(P.muted));
                    None
                }
                Some(Err(err)) => {
                    ui.label(RichText::new(err.as_str()).color(P.danger));
                    None
                }
                Some(Ok(answer)) => Some(answer.clone()),
            };
            if let Some(a) = &answer {
                let mut left = Vec::new();
                if a.already > 0 {
                    left.push(format!("{} credited that way already", thousands(a.already)));
                }
                if a.not_credited > 0 {
                    left.push(format!("{} tracks without those credits", thousands(a.not_credited)));
                }
                if a.not_found > 0 {
                    left.push(format!("{} tracks aren't there anymore", thousands(a.not_found)));
                }
                ui.label(format!("Changes {} tracks and {} albums.", thousands(a.tracks.len()), thousands(a.albums.len())));
                if !left.is_empty() {
                    ui.label(RichText::new(format!("Left as they are: {}.", left.join(", "))).color(P.muted));
                }
            }
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text("Why (optional: kept with every change)").desired_width(360.0));
                let count = answer.as_ref().map(|a| a.tracks.len() + a.albums.len()).unwrap_or(0);
                go = ui.add_enabled(!self.sending && count > 0, egui::Button::new("Credit it so")).clicked();
                cancel = ui.button("Close").clicked();
                if self.sending {
                    ui.label(RichText::new("Working…").color(P.muted));
                }
            });
            match &self.outcome {
                Some(Ok(text)) => {
                    ui.label(RichText::new(text.as_str()).color(P.accent));
                }
                Some(Err(err)) => {
                    ui.label(RichText::new(err.as_str()).color(P.danger));
                }
                None => {}
            }
            if let Some(a) = answer {
                ui.add_space(8.0);
                egui::ScrollArea::vertical().id_salt("credited-as-changes").auto_shrink([false, false]).show(ui, |ui| {
                    if !a.albums.is_empty() {
                        ui.label(RichText::new(format!("Albums ({})", thousands(a.albums.len()))).font(font(13.0, semibold())));
                        for album in &a.albums {
                            if widgets::link(ui, &album.title, font(13.0, semibold()), P.muted) {
                                open = Some(crate::ui::Page::Album(album.id));
                            }
                        }
                        ui.add_space(6.0);
                    }
                    if !a.tracks.is_empty() {
                        ui.label(RichText::new(format!("Tracks ({})", thousands(a.tracks.len()))).font(font(13.0, semibold())));
                        for track in &a.tracks {
                            if widgets::link(ui, &track.title, font(13.0, semibold()), P.muted) {
                                open = Some(crate::ui::Page::Track(track.id));
                            }
                        }
                    }
                });
            }
        });
        let before = self.choice();
        if let Some(artist) = pick_artist
            && self.artist != Some(artist)
        {
            self.artist = Some(artist);
            self.only = Only::All;
            self.to = None;
            self.alias = None;
            shell.artist_details.refresh(artist);
        }
        if let Some(only) = pick_only {
            self.only = only;
        }
        if let Some(to) = pick_to
            && to.as_ref().map(|t| t.0) != self.to.as_ref().map(|t| t.0)
        {
            if let Some((id, _)) = &to {
                shell.artist_details.refresh(*id);
            }
            self.to = to;
            // Another artist's names: chosen again (its own, to start with).
            self.alias = Some(None);
        }
        if let Some(alias) = pick_alias {
            self.alias = Some(alias);
        }
        if self.choice() != before {
            self.outcome = None;
            self.ask(shell);
        }
        if go {
            self.send(shell, false);
        }
        if let Some(page) = open {
            shell.act(Action::Go(page));
            return true;
        }
        (closed || cancel) && !self.sending
    }

    /// The preview for the choice.
    fn ask(&mut self, shell: &Shell) {
        self.loaded = None;
        self.send(shell, true);
    }

    fn send(&mut self, shell: &Shell, preview: bool) {
        let Some(choice) = self.choice() else { return };
        if !preview {
            self.sending = true;
            self.outcome = None;
        }
        let (only_alias, only_own) = match choice.only {
            Only::All => (None, false),
            Only::OwnName => (None, true),
            Only::Alias(id) => (Some(id), false),
        };
        let body = serde_json::json!({
            "track_ids": self.tracks, "only_alias_id": only_alias, "only_own_name": only_own,
            "to_artist_id": choice.to, "alias_id": choice.alias, "reason": self.reason.trim(), "preview": preview,
        });
        let (api, ctx, answer, done) = (shell.api.clone(), shell.ctx.clone(), self.preview.clone(), self.done.clone());
        let from = choice.from;
        shell.runtime.spawn(async move {
            let result = api.send_json::<Answer>(reqwest::Method::POST, &format!("/api/artists/{from}/credited-as"), Some(body)).await.map_err(|e| e.message);
            if preview {
                *answer.lock().unwrap() = Some((choice, result));
            } else {
                *done.lock().unwrap() = Some(result);
            }
            ctx.request_repaint();
        });
    }
}

/// The artists credited on `tracks`, most tracks first, with the names they're credited
/// under now.
fn rows(catalog: &Catalog, tracks: &[Uuid]) -> Vec<Row> {
    let mut by_artist: HashMap<Uuid, (usize, HashMap<Option<String>, usize>)> = HashMap::new();
    for id in tracks {
        let Some(i) = catalog.track_index(*id) else { continue };
        let track = catalog.track_at(i);
        let mut seen = Vec::new();
        for credit in track.credits() {
            if seen.contains(&credit.0) {
                continue;
            }
            seen.push(credit.0);
            let entry = by_artist.entry(credit.0).or_default();
            entry.0 += 1;
            *entry.1.entry(credit.2.as_deref().map(str::to_string)).or_default() += 1;
        }
    }
    let mut rows: Vec<Row> = by_artist
        .into_iter()
        .map(|(artist, (tracks, now))| {
            let mut now: Vec<(Option<String>, usize)> = now.into_iter().collect();
            now.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let name = catalog.artist_label(artist).unwrap_or_else(|| "(unknown)".into());
            Row { artist, name, tracks, now }
        })
        .collect();
    rows.sort_by(|a, b| b.tracks.cmp(&a.tracks).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    rows
}
