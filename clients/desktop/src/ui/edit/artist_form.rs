//! Editing an artist (its page, in edit mode): name, disambiguation (what tells two
//! artists with the same name apart) and other names ("aliases": what tracks can be
//! credited as). One artist revision per save (`PUT /api/artists/{id}`), with the
//! revision the form was filled from, like the track and album forms (a stale save is
//! refused; "Load theirs, keep mine").
//!
//! Other names keep their ids: renaming one renames it on every credit that uses it.
//! Removing one only hides it, and one still credited can't be: × asks the server where
//! it's used (`GET …/aliases/{id}/uses`) and, if anywhere, lists those (as links) instead
//! of removing it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use eframe::egui::{self, Key, Modifiers, RichText};
use serde::Deserialize;
use uuid::Uuid;

use super::SaveState;
use crate::api::ApiError;
use crate::details::ArtistDetail;
use crate::ui::theme::PALETTE as P;
use crate::ui::{widgets, Action, Page, Shell};

#[derive(Debug, Clone, PartialEq)]
pub struct AliasRow {
    /// None: a new one.
    pub id: Option<Uuid>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ArtistData {
    pub name: String,
    pub disambiguation: String,
    pub aliases: Vec<AliasRow>,
}

/// Where an alias is credited, as the server says (only what the user may see, and
/// whether there's more).
#[derive(Debug, Clone, Deserialize)]
pub struct Uses {
    #[serde(default)]
    pub tracks: Vec<TrackUse>,
    #[serde(default)]
    pub albums: Vec<AlbumUse>,
    #[serde(default)]
    pub not_visible: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TrackUse {
    pub track_id: Uuid,
    pub title: String,
    /// The version's label, if not the track's main one.
    pub variant: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AlbumUse {
    pub album_id: Uuid,
    pub title: String,
}

impl Uses {
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty() && self.albums.is_empty() && !self.not_visible
    }
}

/// Where uses are answered: (the alias, where it's credited or why that's not known).
pub type UsesAnswer = Arc<Mutex<Vec<(Uuid, Result<Uses, String>)>>>;

/// Asks where an artist's alias is credited (`GET …/aliases/{id}/uses`); the answer goes
/// into `answer`.
pub fn ask_uses(shell: &Shell, artist: Uuid, alias: Uuid, answer: &UsesAnswer) {
    let (api, answer, ctx) = (shell.api.clone(), answer.clone(), shell.ctx.clone());
    shell.runtime.spawn(async move {
        let path = format!("/api/artists/{artist}/aliases/{alias}/uses");
        let result = api.send_json::<Uses>(reqwest::Method::GET, &path, None).await.map_err(|err| err.message);
        answer.lock().unwrap().push((alias, result));
        ctx.request_repaint();
    });
}

/// An alias still credited: where, as links (change those credits first, or merge).
pub fn uses_links(ui: &mut egui::Ui, shell: &mut Shell, name: &str, uses: &Uses) {
    ui.label(RichText::new(format!("“{name}” is still credited, so it stays. Change these first:")).color(P.muted));
    ui.horizontal_wrapped(|ui| {
        for t in &uses.tracks {
            let text = match &t.variant {
                Some(v) => format!("{} ({v})", t.title),
                None => t.title.clone(),
            };
            if widgets::link(ui, &text, crate::ui::theme::font(13.0, crate::ui::theme::medium()), P.text) {
                shell.act(Action::Go(Page::Track(t.track_id)));
            }
        }
        for a in &uses.albums {
            if widgets::link(ui, &format!("{} (album)", a.title), crate::ui::theme::font(13.0, crate::ui::theme::medium()), P.text) {
                shell.act(Action::Go(Page::Album(a.album_id)));
            }
        }
        if uses.not_visible {
            ui.label(RichText::new("and on some you can't see").color(P.faint));
        }
    });
}

struct Outcome {
    sent: ArtistData,
    revision: Option<Uuid>,
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct Written {
    revision_id: Uuid,
}

pub struct ArtistForm {
    pub artist: Uuid,
    base: Uuid,
    seen: Uuid,
    pub data: ArtistData,
    was: ArtistData,
    reason: String,
    state: SaveState,
    answer: Arc<Mutex<Option<Outcome>>>,
    /// Aliases asked about (×): None while asking; Some(uses) once known (kept: used).
    uses: HashMap<Uuid, Option<Result<Uses, String>>>,
    uses_answer: UsesAnswer,
    /// The library (`Shell::generation`) the uses were asked with: when it changes (a
    /// credit changed somewhere, say one of those listed), they're asked again.
    uses_generation: u64,
    confirm_discard: bool,
    rebase_asked: bool,
    notice: Option<String>,
}

impl ArtistForm {
    pub fn from_detail(d: &ArtistDetail) -> Option<ArtistForm> {
        let base = d.revision_id?;
        let data = ArtistData {
            name: d.name.clone(),
            disambiguation: d.disambiguation.clone(),
            aliases: d.aliases.iter().map(|a| AliasRow { id: Some(a.id), name: a.name.clone() }).collect(),
        };
        Some(ArtistForm {
            artist: d.id,
            base,
            seen: base,
            was: data.clone(),
            data,
            reason: String::new(),
            state: SaveState::Idle,
            answer: Arc::default(),
            uses: HashMap::new(),
            uses_answer: Arc::default(),
            uses_generation: 0,
            confirm_discard: false,
            rebase_asked: false,
            notice: None,
        })
    }

    pub fn dirty(&self) -> bool {
        self.data != self.was
    }

    fn saving(&self) -> bool {
        matches!(self.state, SaveState::Saving)
    }

    pub fn rebase(&mut self, theirs: ArtistForm) {
        fn pick<T: Clone + PartialEq>(mine: &T, was: &T, theirs: &T) -> T {
            if mine != was { mine.clone() } else { theirs.clone() }
        }
        let (m, w, t) = (&self.data, &self.was, &theirs.data);
        let data = ArtistData {
            name: pick(&m.name, &w.name, &t.name),
            disambiguation: pick(&m.disambiguation, &w.disambiguation, &t.disambiguation),
            aliases: pick(&m.aliases, &w.aliases, &t.aliases),
        };
        let reason = std::mem::take(&mut self.reason);
        *self = ArtistForm { data, reason, ..theirs };
        self.notice = Some("Their changes are loaded, with yours on top. Check them, then save.".into());
    }

    fn follow(&mut self, d: &ArtistDetail) {
        let Some(fresh) = ArtistForm::from_detail(d) else { return };
        if fresh.seen == self.seen || self.saving() {
            return;
        }
        self.seen = fresh.seen;
        if !self.dirty() {
            let (reason, state) = (std::mem::take(&mut self.reason), std::mem::take(&mut self.state));
            *self = ArtistForm { reason, state, ..fresh };
        } else if fresh.base != self.base && self.rebase_asked {
            self.rebase(fresh);
        }
    }

    fn behind(&self, d: &ArtistDetail) -> bool {
        d.revision_id != Some(self.base)
    }

    fn discard(&mut self) {
        self.data = self.was.clone();
        self.reason.clear();
        self.uses.clear();
        self.confirm_discard = false;
        self.state = SaveState::Idle;
        self.notice = None;
    }

    fn body(data: &ArtistData, base: Uuid, reason: &str) -> serde_json::Value {
        let aliases: Vec<serde_json::Value> = data
            .aliases
            .iter()
            .filter(|a| !a.name.trim().is_empty())
            .map(|a| match a.id {
                Some(id) => serde_json::json!({ "id": id, "name": a.name.trim() }),
                None => serde_json::json!({ "name": a.name.trim() }),
            })
            .collect();
        serde_json::json!({
            "base_revision_id": base,
            "name": data.name.trim(),
            "disambiguation": data.disambiguation.trim(),
            "aliases": aliases,
            "reason": reason.trim(),
        })
    }

    pub fn save(&mut self, shell: &Shell) {
        if self.saving() || !self.dirty() {
            return;
        }
        if self.data.name.trim().is_empty() {
            self.state = SaveState::Failed("An artist needs a name.".into());
            return;
        }
        self.state = SaveState::Saving;
        self.confirm_discard = false;
        self.notice = None;
        let (api, answer, ctx) = (shell.api.clone(), self.answer.clone(), shell.ctx.clone());
        let (artist, body, sent) = (self.artist, ArtistForm::body(&self.data, self.base, &self.reason), self.data.clone());
        shell.runtime.spawn(async move {
            let result = api.send_json::<Written>(reqwest::Method::PUT, &format!("/api/artists/{artist}"), Some(body)).await;
            let outcome = match result {
                Ok(w) => Outcome { sent, revision: Some(w.revision_id), error: None },
                Err(error) => Outcome { sent, revision: None, error: Some(error) },
            };
            *answer.lock().unwrap() = Some(outcome);
            ctx.request_repaint();
        });
    }

    fn poll(&mut self, shell: &mut Shell) {
        for (alias, result) in std::mem::take(&mut *self.uses_answer.lock().unwrap()) {
            // Used nowhere: it goes (hidden when saved). Else the uses stay shown.
            if result.as_ref().is_ok_and(Uses::is_empty) {
                self.data.aliases.retain(|a| a.id != Some(alias));
                self.uses.remove(&alias);
            } else {
                self.uses.insert(alias, Some(result));
            }
        }
        let Some(outcome) = self.answer.lock().unwrap().take() else { return };
        if let Some(revision) = outcome.revision {
            self.base = revision;
            self.was = outcome.sent;
        }
        self.state = match outcome.error {
            None => {
                self.reason.clear();
                SaveState::Saved(Instant::now())
            }
            Some(err) if err.code == "stale" => SaveState::Stale(err.message),
            Some(err) => SaveState::Failed(err.message),
        };
        shell.artist_details.refresh(self.artist);
        // Names show on tracks and albums: the library catches up.
        shell.act(Action::RefreshLibrary);
    }

    /// × on an existing alias: where is it credited?
    fn ask_uses(&mut self, shell: &Shell, alias: Uuid) {
        self.uses_generation = shell.generation;
        self.uses.insert(alias, None);
        ask_uses(shell, self.artist, alias, &self.uses_answer);
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, d: &ArtistDetail) {
        // The library changed since the uses were listed: asked again (the credits listed
        // may have been changed; an alias used nowhere now goes, as its × asked).
        if self.uses_generation != shell.generation {
            self.uses_generation = shell.generation;
            let listed: Vec<Uuid> = self.uses.iter().filter(|(_, u)| matches!(u, Some(Ok(_)))).map(|(id, _)| *id).collect();
            for alias in listed {
                self.ask_uses(shell, alias);
            }
        }
        self.poll(shell);
        self.follow(d);
        let ctx = ui.ctx().clone();
        // (A popup over the page, a track list's, takes Ctrl+Enter and Esc itself.)
        // (Only with changes: else Ctrl+Enter is another form's, a note's say.)
        if self.dirty() && !shell.modal_was_open && ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter)) {
            self.save(shell);
        }
        if !shell.palette_open && !shell.modal_was_open && !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(Key::Escape)) && self.dirty() {
            if self.confirm_discard {
                self.discard();
            } else {
                self.confirm_discard = true;
            }
        }
        let before = self.data.clone();
        let label = |ui: &mut egui::Ui, text: &str| {
            ui.label(RichText::new(text).color(P.faint));
        };
        egui::Grid::new("artist-form").num_columns(2).spacing([18.0, 8.0]).show(ui, |ui| {
            label(ui, "Name");
            ui.add(egui::TextEdit::singleline(&mut self.data.name).desired_width(420.0));
            ui.end_row();
            label(ui, "Tells apart");
            ui.add(egui::TextEdit::singleline(&mut self.data.disambiguation).hint_text("for two artists with the same name: “US rapper”").desired_width(420.0));
            ui.end_row();
        });
        ui.add_space(8.0);
        ui.label(RichText::new("Other names (what tracks can be credited as; case variants count)").strong());
        let mut ask = None;
        let mut remove_new = None;
        for (i, row) in self.data.aliases.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut row.name).desired_width(300.0).hint_text("exact spelling"));
                let asking = row.id.is_some_and(|id| matches!(self.uses.get(&id), Some(None)));
                if asking {
                    ui.label(RichText::new("checking where it's used…").color(P.faint));
                } else if ui.small_button("×").on_hover_text("Remove (only if nothing's credited as it)").clicked() {
                    match row.id {
                        Some(id) => ask = Some(id),
                        None => remove_new = Some(i),
                    }
                }
            });
            // Still credited: where, as links (change those credits first, or merge).
            if let Some(Some(result)) = row.id.and_then(|id| self.uses.get(&id)) {
                match result {
                    Err(err) => {
                        ui.label(RichText::new(format!("Couldn't check where it's used: {err}")).color(P.danger));
                    }
                    Ok(uses) => uses_links(ui, shell, &row.name, uses),
                }
            }
        }
        if let Some(id) = ask {
            self.ask_uses(shell, id);
        }
        if let Some(i) = remove_new {
            self.data.aliases.remove(i);
        }
        if ui.small_button("+ Add a name").clicked() {
            self.data.aliases.push(AliasRow { id: None, name: String::new() });
        }
        ui.label(RichText::new("Renaming one renames it on every credit that uses it.").color(P.faint).small());
        if self.data != before {
            self.confirm_discard = false;
            if !self.saving() {
                self.state = SaveState::Idle;
            }
        }

        ui.add_space(10.0);
        ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text("Why (optional: kept with the change)").desired_width(560.0));
        let dirty = self.dirty();
        ui.horizontal(|ui| {
            let saving = self.saving();
            if ui.add_enabled(dirty && !saving, egui::Button::new("Save  (Ctrl+Enter)")).clicked() {
                self.save(shell);
            }
            if ui.add_enabled(dirty && !saving, egui::Button::new("Discard")).clicked() {
                self.discard();
            }
            let (text, color) = match &self.state {
                SaveState::Saving => ("Saving…".to_string(), P.muted),
                SaveState::Saved(at) if at.elapsed().as_secs() < 5 && !dirty => ("Saved".to_string(), P.accent),
                SaveState::Failed(err) | SaveState::Stale(err) => (err.clone(), P.danger),
                _ if self.confirm_discard => ("Discard your changes? Esc again to discard them.".to_string(), P.danger),
                _ if dirty => ("Changed, not saved".to_string(), P.muted),
                _ => (String::new(), P.muted),
            };
            ui.label(RichText::new(text).color(color));
        });
        let stale = matches!(self.state, SaveState::Stale(_));
        if dirty && (stale || self.behind(d)) {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Someone else changed this artist since you started.").color(P.muted));
                if self.rebase_asked {
                    ui.label(RichText::new("Loading theirs…").color(P.faint));
                } else if ui.button("Load theirs, keep mine on top").clicked() {
                    match ArtistForm::from_detail(d).filter(|_| self.behind(d)) {
                        Some(fresh) => self.rebase(fresh),
                        None => {
                            self.rebase_asked = true;
                            shell.artist_details.refresh(self.artist);
                        }
                    }
                }
            });
        }
        if let Some(notice) = &self.notice {
            ui.label(RichText::new(notice).color(P.muted));
        }
        if self.dirty() {
            shell.form_dirty = true;
        }
    }

    /// The reason kept with the change (`why:` on the artist page's commandline).
    pub fn set_reason(&mut self, reason: &str) {
        self.reason = reason.to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail(revision: u128, name: &str) -> ArtistDetail {
        serde_json::from_value(serde_json::json!({
            "id": Uuid::from_u128(1), "name": name, "disambiguation": "", "revision_id": Uuid::from_u128(revision),
            "aliases": [{ "id": Uuid::from_u128(5), "name": "Zee" }],
        }))
        .unwrap()
    }

    #[test]
    fn the_body_keeps_ids_and_drops_empty_names() {
        let mut form = ArtistForm::from_detail(&detail(10, "Zed")).unwrap();
        form.data.aliases[0].name = "Zee!".into();
        form.data.aliases.push(AliasRow { id: None, name: "Z".into() });
        form.data.aliases.push(AliasRow { id: None, name: "  ".into() });
        let body = ArtistForm::body(&form.data, form.base, "");
        assert_eq!(body["aliases"], serde_json::json!([{ "id": Uuid::from_u128(5), "name": "Zee!" }, { "name": "Z" }]));
    }

    #[test]
    fn rebasing_keeps_only_the_fields_i_changed() {
        let mut mine = ArtistForm::from_detail(&detail(10, "Zed")).unwrap();
        mine.data.disambiguation = "the rapper".into();
        mine.rebase(ArtistForm::from_detail(&detail(20, "Zedd")).unwrap());
        assert_eq!((mine.data.name.as_str(), mine.data.disambiguation.as_str()), ("Zedd", "the rapper"));
        assert_eq!(mine.base, Uuid::from_u128(20));
    }
}

/// Against a running server (ignored by default): `ACETATE_TEST_SERVER`,
/// `ACETATE_TEST_TOKEN`, `ACETATE_TEST_TRACK` (credited to one artist, as primary; its
/// credit is changed and put back, and its artist gets a name added and removed).
#[cfg(test)]
mod against_a_server {
    use super::*;
    use crate::api::Api;
    use crate::details::TrackDetail;

    #[test]
    #[ignore]
    fn a_name_added_used_and_removed() {
        let vars = ["ACETATE_TEST_SERVER", "ACETATE_TEST_TOKEN", "ACETATE_TEST_TRACK"].map(std::env::var);
        let [Ok(server), Ok(token), Ok(track)] = vars else { return };
        let dir = std::env::temp_dir().join(format!("acetate-artist-form-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), serde_json::json!({"server": server, "token": token, "username": "test"}).to_string()).unwrap();
        let api = Api::new(dir);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            // References (copied into each closure) instead of the values themselves.
            let (api, track) = (&api, track.as_str());
            let get = |path: String| async move { api.send_json::<serde_json::Value>(reqwest::Method::GET, &path, None).await.unwrap() };
            let t: TrackDetail = serde_json::from_value(get(format!("/api/tracks/{track}")).await).unwrap();
            let artist = t.credits[0].artist_id;
            let artist_detail = || async move {
                serde_json::from_value::<ArtistDetail>(get(format!("/api/artists/{artist}")).await).unwrap()
            };
            let put = |body| async move { api.send_json::<Written>(reqwest::Method::PUT, &format!("/api/artists/{artist}"), Some(body)).await };

            // A new name.
            let old = ArtistForm::from_detail(&artist_detail().await).unwrap();
            let mut form = ArtistForm::from_detail(&artist_detail().await).unwrap();
            let alias = format!("Artist form test {}", std::process::id());
            form.data.aliases.push(AliasRow { id: None, name: alias.clone() });
            put(ArtistForm::body(&form.data, form.base, "artist form test")).await.unwrap();
            let d = artist_detail().await;
            let alias_id = d.aliases.iter().find(|a| a.name == alias).expect("listed").id;

            // The track credited as it: listed where it's used, and it can't be removed.
            let credit_as = |alias_id: Option<Uuid>| async move {
                let t: TrackDetail = serde_json::from_value(get(format!("/api/tracks/{track}")).await).unwrap();
                let variant = t.variants.iter().find(|v| Some(v.id) == t.primary_variant_id).unwrap();
                let mut data = variant.data.clone();
                data["credits"][0]["alias_id"] = serde_json::json!(alias_id);
                let body = serde_json::json!({ "base_revision_id": variant.revision_id, "data": data, "reason": "artist form test" });
                api.send_json::<serde_json::Value>(reqwest::Method::PUT, &format!("/api/tracks/{track}/variants/{}", variant.id), Some(body)).await.unwrap();
            };
            credit_as(Some(alias_id)).await;
            let uses: Uses = serde_json::from_value(get(format!("/api/artists/{artist}/aliases/{alias_id}/uses")).await).unwrap();
            assert!(uses.tracks.iter().any(|u| u.track_id.to_string() == track), "{uses:?}");
            let mut form = ArtistForm::from_detail(&artist_detail().await).unwrap();
            form.data.aliases.retain(|a| a.id != Some(alias_id));
            let err = put(ArtistForm::body(&form.data, form.base, "")).await.err().expect("still credited: refused");
            assert!(err.message.contains("still credited"), "{}", err.message);

            // Credited under the name again: now it goes.
            credit_as(None).await;
            let uses: Uses = serde_json::from_value(get(format!("/api/artists/{artist}/aliases/{alias_id}/uses")).await).unwrap();
            assert!(uses.is_empty());
            put(ArtistForm::body(&form.data, form.base, "artist form test (removing it)")).await.unwrap();
            assert!(!artist_detail().await.aliases.iter().any(|a| a.id == alias_id));

            // From the old revision: stale.
            let err = put(ArtistForm::body(&old.data, old.base, "")).await.err().expect("stale");
            assert_eq!(err.code, "stale");
        });
    }
}
