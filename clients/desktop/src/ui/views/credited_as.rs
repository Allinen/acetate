//! "Credited as…" for the marked tracks (edit mode): an artist credited on them (all its
//! credits, or only those under one of its names now), credited under another of its
//! names, or to another artist altogether (a name that was really someone else's), on
//! all of them and on the albums they're on at once. A row per artist credited on the
//! marked tracks says how it's credited now.
//!
//! The commandline (`credited_as_line`) is what the popup does: a command per artist, several
//! at once apart by `|`. The chips below it write into the line (into the command being
//! typed). Each command asks the server what it would change (the exact tracks and albums,
//! nothing kept), and Ctrl+Enter does them in order (a revision each). See the server's
//! `db::credited_as`.

use std::collections::{HashMap, HashSet};

use eframe::egui::{self, RichText, Vec2};
use serde::Deserialize;
use uuid::Uuid;

use crate::catalog::search::Part;
use crate::catalog::Catalog;
use crate::commandline::schema::{self, Kind, Pins};
use super::credited_as_line::{self as line, Credit, SCHEMA};
use crate::ui::edit::commandline::{CommandLine, Pressed};
use crate::ui::edit::inline_search::{Choice as Hit, InlineSearch};
use crate::ui::task::Task;
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
pub enum Only {
    All,
    OwnName,
    Alias(Uuid),
}

/// What a preview was asked for (the reason doesn't change it).
type Asked = (Uuid, Only, Uuid, Option<Uuid>);

fn asked(c: &Credit) -> Asked {
    (c.from, c.only, c.to, c.alias)
}

/// A command's preview: what the server says it would change.
struct Preview {
    asked: Asked,
    task: Task<Result<Answer, String>>,
    answer: Option<Result<Answer, String>>,
}

pub struct CreditedAs {
    tracks: Vec<Uuid>,
    rows: Vec<Row>,
    /// The library the rows were counted from.
    counted_for: u64,
    search: InlineSearch,
    /// One per command that can be done, in the line's order.
    previews: Vec<Preview>,
    sending: bool,
    done: Task<Result<(usize, usize), String>>,
    /// What the last Ctrl+Enter did (or why not).
    outcome: Option<Result<String, String>>,
    cl: CommandLine,
    /// Names chosen in the line's list.
    pins: Pins,
    /// Artists whose names were fetched afresh for this popup.
    fetched: HashSet<Uuid>,
    notice: Option<String>,
    /// The line was filled in for the opening: the artist alone credited on the tracks, or
    /// the artist and alias given (once its names are there).
    started: bool,
    alias: Option<(Uuid, Uuid)>,
}

impl CreditedAs {
    pub fn open(tracks: Vec<Uuid>) -> CreditedAs {
        CreditedAs {
            tracks,
            rows: Vec::new(),
            counted_for: u64::MAX,
            search: InlineSearch::new(Part::Artists),
            previews: Vec::new(),
            sending: false,
            done: Task::default(),
            outcome: None,
            cl: {
                let mut cl = CommandLine::new(egui::Id::new("credited-as-line"));
                cl.focus = true;
                cl
            },
            pins: Pins::default(),
            fetched: HashSet::new(),
            notice: None,
            started: false,
            alias: None,
        }
    }

    /// For the credits under one of an artist's other names: `tracks` are where it's used.
    /// The artist and the credits are chosen already; what they become is left to say.
    pub fn for_alias(tracks: Vec<Uuid>, artist: Uuid, alias: Uuid) -> CreditedAs {
        CreditedAs { alias: Some((artist, alias)), ..CreditedAs::open(tracks) }
    }

    /// The popup. True when it's closed.
    pub fn show(&mut self, shell: &mut Shell) -> bool {
        let Some(catalog) = shell.catalog.value.clone() else { return true };
        if self.counted_for != shell.generation {
            self.counted_for = shell.generation;
            self.rows = rows(&catalog, &self.tracks);
        }
        let artist_list: Vec<(Uuid, String)> = self.rows.iter().map(|r| (r.artist, r.name.clone())).collect();
        if !self.started {
            self.start(shell, &artist_list);
        }
        // The names of the artists the line names (the artist, the one credited to), fetched
        // afresh once.
        let parsed = schema::parse(&SCHEMA, &self.cl.text);
        let no_names = HashMap::new();
        let named = line::World { catalog: &catalog, artists: &artist_list, names: &no_names, pins: &self.pins }.named(&parsed);
        let mut names: HashMap<Uuid, Vec<(Uuid, String)>> = HashMap::new();
        for id in named {
            if self.fetched.insert(id) {
                shell.artist_details.open(id);
            }
            if let Some(d) = shell.artist_details.get(id).value.as_ref() {
                names.insert(id, d.aliases.iter().map(|a| (a.id, a.name.clone())).collect());
            }
        }
        // (A copy: taking a row pins a name, read from the next frame on.)
        let pins = self.pins.clone();
        let world = line::World { catalog: &catalog, artists: &artist_list, names: &names, pins: &pins };
        let done: Vec<Result<(String, Credit), String>> = parsed.commands.iter().map(|c| world.resolve(c)).collect();
        let credits: Vec<Credit> = done.iter().filter_map(|d| d.as_ref().ok().map(|(_, c)| c.clone())).collect();
        self.preview(shell, &credits);
        if let Some(result) = self.done.poll() {
            self.sending = false;
            shell.act(Action::RefreshLibrary);
            self.outcome = Some(result.map(|(t, a)| format!("Done: {} tracks and {} albums credited anew.", thousands(t), thousands(a))));
            // Asked again: what's left to change is nothing now.
            self.previews.clear();
        }
        let answers: Vec<Option<Result<Answer, String>>> = done
            .iter()
            .map(|d| d.as_ref().ok().and_then(|(_, c)| self.previews.iter().find(|p| p.asked == asked(c))).and_then(|p| p.answer.clone()))
            .collect();
        let total: usize = answers.iter().flatten().filter_map(|a| a.as_ref().ok()).map(|a| a.tracks.len() + a.albums.len()).sum();
        let all_ready = !done.is_empty() && done.iter().all(Result::is_ok) && answers.iter().all(|a| matches!(a, Some(Ok(_))));
        let can_go = !self.sending && all_ready && total > 0;

        // The command being typed: the chips are its parts.
        let current = parsed.current();
        let parts = current.map(|c| world.parts(c));
        let ctrl_enter = shell.ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
        shell.modal_open = true;
        let ctx = shell.ctx.clone();
        let title = format!("Credited as… on {} tracks", thousands(self.tracks.len()));
        // What a chip writes into the line: (key, value, pinned as).
        let mut write: Option<(&'static str, String, Option<(Kind, Uuid)>)> = None;
        let mut go = false;
        let mut cancel = false;
        let mut open = None;
        let closed = crate::ui::modal::show(&ctx, "credited-as", &title, Vec2::new(900.0, 780.0), |ui| {
            ui.label("An artist's credits on these tracks, and on the albums they're on: under another of its names, or credited to another artist (a name that was really someone else's). A revision each that changes; other artists' credits stay as they are.");
            ui.add_space(6.0);
            let focused = self.cl.hold_focus(&ctx);
            let list_rows = schema::rows(&SCHEMA, &parsed, &self.cl.text, &world);
            self.cl.highlighted = self.cl.highlighted.min(list_rows.len().saturating_sub(1));
            let pressed = if focused { self.cl.keys(&ctx, list_rows.len(), false, false) } else { Pressed::default() };
            let hint = "artist:Name  only:  to:  as:  why:   ( | another artist · Ctrl+Enter credits them so)";
            if self.cl.field(ui, true, hint).changed() {
                self.notice = None;
            }
            let driven = self.cl.drive(ui, pressed, &list_rows, &SCHEMA, &mut self.pins, "↑/↓ choose · Tab or Enter: take it");
            if driven.took {
                self.notice = None;
            }
            if driven.run {
                self.notice = Some(match done.iter().find_map(|d| d.as_ref().err()) {
                    Some(err) => format!("Not yet: {err}"),
                    None if done.is_empty() => "Type artist: and what its credits become.".to_string(),
                    None => "Ctrl+Enter credits them so.".to_string(),
                });
            }
            if let Some(notice) = &self.notice {
                ui.label(RichText::new(notice).color(P.muted).small());
            }

            // What each command does, and what the server says it changes.
            if !done.is_empty() {
                ui.add_space(4.0);
                egui::Frame::NONE.fill(P.bg).corner_radius(8).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
                    for (n, (d, answer)) in done.iter().zip(&answers).enumerate() {
                        let number = if done.len() > 1 { format!("{}. ", n + 1) } else { String::new() };
                        match d {
                            Err(err) => {
                                ui.label(RichText::new(format!("{number}{err}")).color(P.danger));
                            }
                            Ok((label, _)) => {
                                ui.label(RichText::new(format!("{number}{label}")).color(P.accent));
                                match answer {
                                    None => {
                                        ui.label(RichText::new("  Asking the server what would change…").color(P.muted).small());
                                    }
                                    Some(Err(err)) => {
                                        ui.label(RichText::new(format!("  {err}")).color(P.danger).small());
                                    }
                                    Some(Ok(a)) => {
                                        ui.label(RichText::new(format!("  {}", changes(a))).small());
                                    }
                                }
                            }
                        }
                    }
                });
            }

            if let Some(parts) = &parts {
                self.chips(ui, shell, &catalog, parts, &names, &mut write);
            } else if !self.rows.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(if parsed.commands.is_empty() { "Which artist" } else { "Another artist" }).font(font(14.0, semibold())));
                ui.horizontal_wrapped(|ui| {
                    for row in &self.rows {
                        if widgets::chip(ui, &format!("{}  {}", row.name, thousands(row.tracks)), false) {
                            write = Some(("artist", row.name.clone(), Some((Kind::Artist, row.artist))));
                        }
                    }
                });
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                go = ui.add_enabled(can_go, egui::Button::new("Credit it so  (Ctrl+Enter)")).clicked();
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
            let shown: Vec<(&str, &Answer)> = done.iter().zip(&answers).filter_map(|(d, a)| Some((d.as_ref().ok()?.0.as_str(), a.as_ref()?.as_ref().ok()?))).filter(|(_, a)| !a.tracks.is_empty() || !a.albums.is_empty()).collect();
            if !shown.is_empty() {
                ui.add_space(8.0);
                egui::ScrollArea::vertical().id_salt("credited-as-changes").auto_shrink([false, false]).show(ui, |ui| {
                    for (label, a) in &shown {
                        if shown.len() > 1 {
                            ui.label(RichText::new(*label).font(font(13.5, semibold())).color(P.accent));
                        }
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
                        ui.add_space(8.0);
                    }
                });
            }
        });
        if let Some((key, value, pin)) = write {
            self.write(key, &value, pin);
            self.outcome = None;
        }
        if go || (ctrl_enter && can_go) {
            self.send(shell, &credits);
        } else if ctrl_enter && !self.sending {
            self.notice = Some(match done.iter().find_map(|d| d.as_ref().err()) {
                Some(err) => format!("Not done: {err}"),
                None if done.is_empty() => "Nothing to do yet: type artist: and what its credits become.".to_string(),
                None if !all_ready => "Not done: still asking the server what would change.".to_string(),
                None => "Nothing would change.".to_string(),
            });
        }
        if let Some(page) = open {
            shell.act(Action::Go(page));
            return true;
        }
        (closed || cancel) && !self.sending
    }

    /// The line filled in for the opening, once what it needs is there.
    fn start(&mut self, shell: &mut Shell, artists: &[(Uuid, String)]) {
        let q = |v: &str| SCHEMA.quoted(v);
        match self.alias {
            Some((artist, alias)) => {
                let Some(name) = shell.catalog.value.as_ref().and_then(|c| c.artist_label(artist)) else { return };
                let Some(details) = shell.artist_details.get(artist).value.clone() else { return };
                self.started = true;
                self.pins.set(Kind::Artist, &name, artist);
                self.cl.text = match details.aliases.iter().find(|a| a.id == alias) {
                    Some(a) => {
                        self.pins.set(Kind::AliasOf(&[]), &a.name, alias);
                        format!("artist:{} only:{} ", q(&name), q(&a.name))
                    }
                    None => format!("artist:{} ", q(&name)),
                };
            }
            None if self.rows.is_empty() => {}
            None => {
                self.started = true;
                if let [(artist, name)] = artists {
                    self.pins.set(Kind::Artist, name, *artist);
                    self.cl.text = format!("artist:{} ", q(name));
                }
            }
        }
        self.cl.place_cursor = true;
    }

    /// The chips for the command being typed: its artist, which credits, to whom, as what.
    /// One clicked is written into the line.
    fn chips(&mut self, ui: &mut egui::Ui, shell: &Shell, catalog: &Catalog, parts: &line::Parts, names: &HashMap<Uuid, Vec<(Uuid, String)>>, write: &mut Option<(&'static str, String, Option<(Kind, Uuid)>)>) {
        let from = parts.from.as_ref().ok().map(|f| f.0);
        ui.add_space(6.0);
        ui.label(RichText::new("Which artist").font(font(14.0, semibold())));
        egui::ScrollArea::vertical().id_salt("credited-as-artists").max_height(160.0).show(ui, |ui| {
            for row in &self.rows {
                ui.horizontal_wrapped(|ui| {
                    if widgets::chip(ui, &format!("{}  {}", row.name, thousands(row.tracks)), from == Some(row.artist)) {
                        *write = Some(("artist", row.name.clone(), Some((Kind::Artist, row.artist))));
                    }
                    let now: Vec<String> = row.now.iter().map(|(name, n)| format!("{} {}", name.as_deref().map(|a| format!("as “{a}”")).unwrap_or_else(|| "own name".into()), thousands(*n))).collect();
                    ui.label(RichText::new(now.join(" · ")).color(P.muted));
                });
            }
        });
        let Some(artist) = from else { return };
        let Some(row) = self.rows.iter().find(|r| r.artist == artist) else { return };
        let no_names = Vec::new();
        let from_names = names.get(&artist).unwrap_or(&no_names);

        // Which of its credits: all, or those under one of its names now.
        let only = match &parts.only {
            None => Some(Only::All),
            Some(r) => r.as_ref().ok().copied(),
        };
        ui.add_space(8.0);
        ui.label(RichText::new("Which credits").font(font(14.0, semibold())));
        ui.horizontal_wrapped(|ui| {
            if widgets::chip(ui, &format!("all {}", thousands(row.tracks)), only == Some(Only::All)) {
                *write = Some(("only", "all".into(), None));
            }
            for (now, n) in &row.now {
                match now {
                    None => {
                        if widgets::chip(ui, &format!("under its own name {}", thousands(*n)), only == Some(Only::OwnName)) {
                            *write = Some(("only", "own".into(), None));
                        }
                    }
                    Some(alias) => match from_names.iter().find(|(_, a)| a == alias) {
                        Some((id, _)) => {
                            if widgets::chip(ui, &format!("as “{alias}” {}", thousands(*n)), only == Some(Only::Alias(*id))) {
                                *write = Some(("only", alias.clone(), Some((Kind::AliasOf(&[]), *id))));
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
        let to = match &parts.to {
            None => Some(None),
            Some(r) => r.as_ref().ok().cloned(),
        };
        ui.add_space(8.0);
        ui.label(RichText::new("Credit to").font(font(14.0, semibold())));
        ui.horizontal_wrapped(|ui| {
            if widgets::chip(ui, &format!("{} (the same artist)", row.name), to == Some(None)) {
                *write = Some(("to", "same".into(), None));
            }
            if let Some(Some((_, other))) = &to {
                widgets::chip(ui, other, true);
            }
        });
        let hit = self.search.show(ui, shell, Some(catalog), egui::Id::new("credited-as-to"), "another artist: type its name", &[]);
        if let Some((Hit::Hit(i), _)) = hit
            && let Some(a) = catalog.artists.get(i as usize)
        {
            *write = Some(("to", a.name.clone(), Some((Kind::Artist, a.id))));
        }
        let Some(to) = to else { return };
        let (whose, to_name) = to.unwrap_or((artist, row.name.clone()));
        let to_names = names.get(&whose).unwrap_or(&no_names);

        ui.add_space(8.0);
        ui.label(RichText::new("As").font(font(14.0, semibold())));
        let alias = parts.alias.as_ref().and_then(|r| r.as_ref().ok().copied());
        ui.horizontal_wrapped(|ui| {
            if widgets::chip(ui, &format!("{to_name} (its own name)"), alias == Some(None)) {
                *write = Some(("as", "own".into(), None));
            }
            for (id, name) in to_names {
                if widgets::chip(ui, name, alias == Some(Some(*id))) {
                    *write = Some(("as", name.clone(), Some((Kind::AliasOf(&[]), *id))));
                }
            }
            if to_names.is_empty() {
                ui.label(RichText::new("No other names: add one on its page (edit mode).").color(P.faint));
            }
        });
    }

    /// A chip's choice, written into the command being typed (a new one for an artist when
    /// none is), and the cursor after it.
    fn write(&mut self, key: &str, value: &str, pin: Option<(Kind, Uuid)>) {
        if let Some((kind, id)) = pin {
            self.pins.set(kind, value, id);
        }
        let parsed = schema::parse(&SCHEMA, &self.cl.text);
        let quoted = SCHEMA.quoted(value);
        let text = &self.cl.text;
        self.cl.text = match (key, parsed.current()) {
            ("artist", Some(c)) => format!("{}{quoted}{}", &text[..c.value_start], &text[c.end..]),
            ("artist", None) => {
                let base = text.trim_end();
                if base.is_empty() { format!("artist:{quoted}") } else { format!("{base} artist:{quoted}") }
            }
            (_, Some(_)) => schema::set_arg(&SCHEMA, text, parsed.commands.len() - 1, key, value),
            (_, None) => text.clone(),
        };
        if !self.cl.text.ends_with(' ') {
            self.cl.text.push(' ');
        }
        self.cl.place_cursor = true;
    }

    /// A preview for each command that can be done: asked for those not asked yet, kept
    /// for those that are.
    fn preview(&mut self, shell: &Shell, credits: &[Credit]) {
        let mut kept = std::mem::take(&mut self.previews);
        for c in credits {
            let wanted = asked(c);
            if self.previews.iter().any(|p| p.asked == wanted) {
                continue;
            }
            let mut p = match kept.iter().position(|p| p.asked == wanted) {
                Some(i) => kept.swap_remove(i),
                None => {
                    let mut p = Preview { asked: wanted, task: Task::default(), answer: None };
                    let (from, body) = request(c, &self.tracks, true);
                    p.task.start_with(shell, |api| async move { ask(api, from, body).await });
                    p
                }
            };
            if let Some(answer) = p.task.poll() {
                p.answer = Some(answer);
            }
            self.previews.push(p);
        }
    }

    /// Does every command, in order; stops at the first that fails.
    fn send(&mut self, shell: &Shell, credits: &[Credit]) {
        self.sending = true;
        self.outcome = None;
        self.notice = None;
        let calls: Vec<(Uuid, serde_json::Value)> = credits.iter().map(|c| request(c, &self.tracks, false)).collect();
        let many = calls.len() > 1;
        self.done.send_with(shell, move |api| async move {
            let (mut tracks, mut albums) = (0, 0);
            for (n, (from, body)) in calls.into_iter().enumerate() {
                match ask(api.clone(), from, body).await {
                    Ok(a) => {
                        tracks += a.tracks.len();
                        albums += a.albums.len();
                    }
                    Err(e) if many && n > 0 => return Err(format!("Command {} not done: {e} (those before it are done)", n + 1)),
                    Err(e) => return Err(e),
                }
            }
            Ok((tracks, albums))
        });
    }
}

/// The request for a credit: the artist's id (in the path), and the body.
fn request(c: &Credit, tracks: &[Uuid], preview: bool) -> (Uuid, serde_json::Value) {
    let (only_alias, only_own) = match c.only {
        Only::All => (None, false),
        Only::OwnName => (None, true),
        Only::Alias(id) => (Some(id), false),
    };
    let body = serde_json::json!({
        "track_ids": tracks, "only_alias_id": only_alias, "only_own_name": only_own,
        "to_artist_id": c.to, "alias_id": c.alias, "reason": c.why.trim(), "preview": preview,
    });
    (c.from, body)
}

async fn ask(api: crate::api::Api, from: Uuid, body: serde_json::Value) -> Result<Answer, String> {
    api.send_json::<Answer>(reqwest::Method::POST, &format!("/api/artists/{from}/credited-as"), Some(body)).await.map_err(|e| e.message)
}

/// What a preview says changes, and what's left as it is.
fn changes(a: &Answer) -> String {
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
    let mut out = format!("Changes {} tracks and {} albums.", thousands(a.tracks.len()), thousands(a.albums.len()));
    if !left.is_empty() {
        out.push_str(&format!(" Left as they are: {}.", left.join(", ")));
    }
    out
}

/// The artists credited on `tracks`, most tracks first, with the names they're credited
/// under now.
fn rows(catalog: &Catalog, tracks: &[Uuid]) -> Vec<Row> {
    let mut by_artist: HashMap<Uuid, (usize, HashMap<Option<String>, usize>)> = HashMap::new();
    for id in tracks {
        let Some(i) = catalog.track_index(*id) else { continue };
        let track = catalog.track_at(i);
        let mut seen = Vec::new();
        for credit in track.credits().iter() {
            if seen.contains(&credit.0) {
                continue;
            }
            seen.push(credit.0);
            let entry = by_artist.entry(credit.0).or_default();
            entry.0 += 1;
            *entry.1.entry(credit.2.map(str::to_string)).or_default() += 1;
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
