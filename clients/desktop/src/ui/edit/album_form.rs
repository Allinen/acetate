//! Editing an album (its page, in edit mode): title, kind, release date and credits,
//! saved as one album revision (`PUT /api/albums/{id}`) with the revision the form was
//! filled from: a change someone made in between is refused
//! (`stale`) instead of undone, and "Load theirs, keep mine" takes theirs with only the
//! fields you changed put back. Ctrl+Enter saves; Esc (outside a field) asks, and a
//! second Esc discards. New artists typed in the credits are made when it's saved.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use eframe::egui::{self, Key, Modifiers, RichText};
use uuid::Uuid;

use super::credits::{self, CreditRow, CreditsEditor};
use super::{capitalized, combo, PickKind, SaveState};
use crate::api::ApiError;
use crate::catalog::Catalog;
use crate::details::AlbumDetail;
use crate::ui::theme::PALETTE as P;
use crate::ui::{Action, Shell};

pub use acetate_core::editor::forms::AlbumData;

/// How a save went: what was sent (new artists' ids filled in), the new revision, or why
/// not.
struct Outcome {
    sent: AlbumData,
    revision: Option<Uuid>,
    error: Option<ApiError>,
}

pub struct AlbumForm {
    pub album: Uuid,
    /// The revision the form was filled from (sent with a save), and the one the details
    /// had last time they were looked at.
    base: Uuid,
    seen: Uuid,
    pub data: AlbumData,
    was: AlbumData,
    reason: String,
    state: SaveState,
    answer: Arc<Mutex<Option<Outcome>>>,
    credits_editor: CreditsEditor,
    /// The credit row an artist is being picked for (the palette).
    awaiting: Option<usize>,
    confirm_discard: bool,
    rebase_asked: bool,
    notice: Option<String>,
}

#[derive(serde::Deserialize)]
struct Written {
    revision_id: Uuid,
}

impl AlbumForm {
    /// Filled from the album's details. None without a revision to build on.
    pub fn from_detail(d: &AlbumDetail) -> Option<AlbumForm> {
        let base = d.revision_id?;
        let data = AlbumData {
            title: d.title.clone(),
            kind: d.kind.clone(),
            release_date: d.release_date.clone().unwrap_or_default(),
            credits: credits::rows_from(&d.credits),
        };
        Some(AlbumForm {
            album: d.id,
            base,
            seen: base,
            was: data.clone(),
            data,
            reason: String::new(),
            state: SaveState::Idle,
            answer: Arc::default(),
            credits_editor: CreditsEditor::default(),
            awaiting: None,
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

    /// Takes `theirs` as the base, with the fields you changed put back on top (the
    /// credits as a whole).
    pub fn rebase(&mut self, theirs: AlbumForm) {
        fn pick<T: Clone + PartialEq>(mine: &T, was: &T, theirs: &T) -> T {
            if mine != was { mine.clone() } else { theirs.clone() }
        }
        let (m, w, t) = (&self.data, &self.was, &theirs.data);
        let data = AlbumData {
            title: pick(&m.title, &w.title, &t.title),
            kind: pick(&m.kind, &w.kind, &t.kind),
            release_date: pick(&m.release_date, &w.release_date, &t.release_date),
            credits: pick(&m.credits, &w.credits, &t.credits),
        };
        let reason = std::mem::take(&mut self.reason);
        let credits_editor = std::mem::take(&mut self.credits_editor);
        *self = AlbumForm { data, reason, credits_editor, ..theirs };
        self.notice = Some("Their changes are loaded, with yours on top. Check them, then save.".into());
    }

    /// Follows the details: without changes, the newest; with changes, kept (and rebased
    /// if "Load theirs" was asked).
    fn follow(&mut self, d: &AlbumDetail) {
        let Some(fresh) = AlbumForm::from_detail(d) else { return };
        if fresh.seen == self.seen || self.saving() {
            return;
        }
        self.seen = fresh.seen;
        if !self.dirty() {
            let (reason, state) = (std::mem::take(&mut self.reason), std::mem::take(&mut self.state));
            let credits_editor = std::mem::take(&mut self.credits_editor);
            *self = AlbumForm { reason, state, credits_editor, ..fresh };
        } else if fresh.base != self.base && self.rebase_asked {
            self.rebase(fresh);
        }
    }

    fn behind(&self, d: &AlbumDetail) -> bool {
        d.revision_id != Some(self.base)
    }

    fn discard(&mut self) {
        self.data = self.was.clone();
        self.reason.clear();
        self.confirm_discard = false;
        self.state = SaveState::Idle;
        self.notice = None;
    }

    fn body(data: &AlbumData, base: Uuid, reason: &str) -> serde_json::Value {
        acetate_core::editor::forms::album_body(data, base, reason)
    }

    pub fn save(&mut self, shell: &Shell) {
        if self.saving() || !self.dirty() {
            return;
        }
        if self.data.title.trim().is_empty() {
            self.state = SaveState::Failed("An album needs a title.".into());
            return;
        }
        self.state = SaveState::Saving;
        self.confirm_discard = false;
        self.notice = None;
        let (api, answer, ctx) = (shell.api.clone(), self.answer.clone(), shell.ctx.clone());
        let (album, base, reason, mut data) = (self.album, self.base, self.reason.clone(), self.data.clone());
        shell.runtime.spawn(async move {
            let made = credits::make_new_artists(&api, &mut data.credits).await;
            let outcome = match made {
                Err(error) => Outcome { sent: data, revision: None, error: Some(error) },
                Ok(()) => {
                    let body = AlbumForm::body(&data, base, &reason);
                    match api.send_json::<Written>(reqwest::Method::PUT, &format!("/api/albums/{album}"), Some(body)).await {
                        Ok(w) => Outcome { sent: data, revision: Some(w.revision_id), error: None },
                        Err(error) => Outcome { sent: data, revision: None, error: Some(error) },
                    }
                }
            };
            *answer.lock().unwrap() = Some(outcome);
            ctx.request_repaint();
        });
    }

    fn poll(&mut self, shell: &mut Shell) {
        let Some(outcome) = self.answer.lock().unwrap().take() else { return };
        credits::take_ids(&mut self.data.credits, &outcome.sent.credits);
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
        shell.album_details.refresh(self.album);
        shell.act(Action::RefreshLibrary);
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: Option<&Catalog>, d: &AlbumDetail) {
        self.poll(shell);
        self.follow(d);
        if let Some(i) = self.awaiting
            && let Some(picked) = shell.picker.take()
        {
            self.awaiting = None;
            if let (PickKind::Artist, Some(row)) = (picked.kind, self.data.credits.get_mut(i)) {
                *row = CreditRow { artist_id: picked.id, name: picked.name, alias_id: None, alias: None, ..row.clone() };
            }
        }
        let ctx = ui.ctx().clone();
        // (A popup over the page, a track list's, takes Ctrl+Enter and Esc itself. Without
        // changes it's left for what's below: a note being written.)
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

        let vocab = shell.account.vocab.clone().unwrap_or_default();
        let before = self.data.clone();
        let label = |ui: &mut egui::Ui, text: &str| {
            ui.label(RichText::new(text).color(P.faint));
        };
        egui::Grid::new("album-form").num_columns(2).spacing([18.0, 8.0]).show(ui, |ui| {
            label(ui, "Title");
            ui.add(egui::TextEdit::singleline(&mut self.data.title).desired_width(420.0));
            ui.end_row();
            label(ui, "Kind");
            combo(ui, "album-kind", &mut self.data.kind, &vocab.album_kinds, capitalized);
            ui.end_row();
            label(ui, "Released");
            ui.add(egui::TextEdit::singleline(&mut self.data.release_date).hint_text("as the source wrote it: 2019, 2019-05-12…").desired_width(420.0));
            ui.end_row();
        });
        ui.add_space(8.0);
        ui.label(RichText::new("Album artists").strong());
        let field = egui::Id::new("album-form-artist");
        if let Some(i) = self.credits_editor.show(ui, shell, catalog, "album-form", field, &mut self.data.credits, &vocab.credit_roles) {
            self.awaiting = Some(i);
            shell.act(Action::Pick { kind: PickKind::Artist, except: None });
        }
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
                ui.label(RichText::new("Someone else changed this album since you started.").color(P.muted));
                if self.rebase_asked {
                    ui.label(RichText::new("Loading theirs…").color(P.faint));
                } else if ui.button("Load theirs, keep mine on top").clicked() {
                    match AlbumForm::from_detail(d).filter(|_| self.behind(d)) {
                        Some(fresh) => self.rebase(fresh),
                        None => {
                            self.rebase_asked = true;
                            shell.album_details.refresh(self.album);
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

    /// Why (kept with the change), as the commandline's `why:` says it.
    pub fn set_reason(&mut self, reason: &str) {
        self.reason = reason.to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail(revision: u128, title: &str, kind: &str) -> AlbumDetail {
        serde_json::from_value(serde_json::json!({
            "id": Uuid::from_u128(1), "title": title, "kind": kind, "release_date": null,
            "revision_id": Uuid::from_u128(revision),
        }))
        .unwrap()
    }

    #[test]
    fn rebasing_keeps_only_the_fields_i_changed() {
        let mut mine = AlbumForm::from_detail(&detail(10, "Old", "album")).unwrap();
        mine.data.title = "My title".into();
        mine.rebase(AlbumForm::from_detail(&detail(20, "Their title", "ep")).unwrap());
        assert_eq!((mine.data.title.as_str(), mine.data.kind.as_str()), ("My title", "ep"));
        assert_eq!(mine.base, Uuid::from_u128(20));
        assert!(mine.dirty());
    }

    #[test]
    fn the_body_is_what_the_server_takes() {
        let mut form = AlbumForm::from_detail(&detail(10, "T", "album")).unwrap();
        form.data.release_date = "  ".into();
        credits::add_artist(&mut form.data.credits, Some(Uuid::from_u128(7)), "A".into(), false);
        let body = AlbumForm::body(&form.data, form.base, " why ");
        assert!(body["release_date"].is_null());
        assert_eq!(body["credits"][0]["role"], "primary");
        assert_eq!(body["reason"], "why");
        assert_eq!(body["base_revision_id"], Uuid::from_u128(10).to_string());
    }
}

/// Against a running server (ignored by default): `ACETATE_TEST_SERVER`,
/// `ACETATE_TEST_TOKEN`, `ACETATE_TEST_ALBUM` (edited, then put back; a throwaway database:
/// it makes an artist).
#[cfg(test)]
mod against_a_server {
    use super::*;
    use crate::api::Api;

    #[test]
    #[ignore]
    fn an_album_save_and_a_stale_one() {
        let vars = ["ACETATE_TEST_SERVER", "ACETATE_TEST_TOKEN", "ACETATE_TEST_ALBUM"].map(std::env::var);
        let [Ok(server), Ok(token), Ok(album)] = vars else { return };
        let dir = std::env::temp_dir().join(format!("acetate-album-form-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), serde_json::json!({"server": server, "token": token, "username": "test"}).to_string()).unwrap();
        let api = Api::new(dir);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (api, album) = (&api, album.as_str());
            let detail = move || async move { api.send_json::<AlbumDetail>(reqwest::Method::GET, &format!("/api/albums/{album}"), None).await.unwrap() };
            let put = move |body| async move { api.send_json::<Written>(reqwest::Method::PUT, &format!("/api/albums/{album}"), Some(body)).await };
            let old = AlbumForm::from_detail(&detail().await).unwrap();

            // A new title, and a new artist typed in (made first, as a save does).
            let mut form = AlbumForm::from_detail(&detail().await).unwrap();
            form.data.title = format!("{} (album form test)", old.data.title);
            let name = format!("Album form test artist {}", std::process::id());
            credits::add_artist(&mut form.data.credits, None, name.clone(), true);
            credits::make_new_artists(api, &mut form.data.credits).await.unwrap();
            put(AlbumForm::body(&form.data, form.base, "album form test")).await.unwrap();
            let d = detail().await;
            assert_eq!(d.title, form.data.title);
            assert!(d.credits.iter().any(|c| c.artist_name == name && c.role == "featured"), "{:?}", d.credits);

            // From the old revision: stale.
            let err = put(AlbumForm::body(&old.data, old.base, "")).await.err().expect("a stale save is refused");
            assert_eq!(err.code, "stale");

            // Put back (the artist stays credited: taking it off is an edit like any other).
            let mut back = AlbumForm::from_detail(&detail().await).unwrap();
            back.data.title = old.data.title.clone();
            put(AlbumForm::body(&back.data, back.base, "album form test (putting it back)")).await.unwrap();
        });
    }
}
