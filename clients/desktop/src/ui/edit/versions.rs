//! A track's versions (variants), in edit mode: named sets of metadata ("File tags", "Leak
//! title"…), one of them shown (the track's title, artists and the rest come from it). The
//! bar above the track's editor says which one it edits and changes that; makes a new one
//! (a copy of the one being edited, to change on its own); renames one; makes one the
//! shown one. Versions aren't removed (nothing is), only no longer shown.
//!
//! Switching and copying wait for the editor's staged changes to be applied or discarded:
//! a copy is of what's saved, and the changes are for the version they were typed for.
//! The track page's commandline does the same (`version:`, see `views::track_commands`).


use eframe::egui::{self, Key, RichText};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::details::{TrackDetail, Variant};
use crate::ui::task::Task;
use crate::ui::theme::PALETTE as P;
use crate::ui::{Action, Shell};

const NAME_ID: &str = "version-name";

/// What's being named: a new version, or one being renamed.
enum Naming {
    New(String),
    Rename(Uuid, String),
}

/// What a request did, once the server said yes.
enum Done {
    Created(Uuid),
    Renamed,
    Shown,
}

#[derive(Default)]
pub struct VersionsBar {
    track: Option<Uuid>,
    naming: Option<Naming>,
    busy: bool,
    answer: Task<Result<Done, String>>,
    /// A version just made: the editor edits it once the details have it.
    switch_to: Option<Uuid>,
    error: Option<String>,
}

/// A version's name as shown (the server doesn't allow empty ones, but old ones may be).
fn name(v: &Variant) -> &str {
    if v.label.trim().is_empty() { "(no name)" } else { &v.label }
}

impl VersionsBar {
    /// The bar. `editing`: the version the editor edits; `changed`: it has changes staged.
    /// Returns the version to edit instead, when one was chosen (or just made).
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, d: &TrackDetail, editing: Uuid, changed: bool) -> Option<Uuid> {
        if self.track != Some(d.id) {
            *self = VersionsBar { track: Some(d.id), ..Default::default() };
        }
        if let Some(result) = self.answer.poll() {
            self.busy = false;
            match result {
                Ok(done) => {
                    self.error = None;
                    shell.track_details.refresh(d.id);
                    match done {
                        Done::Created(id) => self.switch_to = Some(id),
                        Done::Renamed => {}
                        // The track shows another version's title, artists…: the lists too.
                        Done::Shown => shell.act(Action::RefreshLibrary),
                    }
                }
                Err(err) => self.error = Some(err),
            }
        }
        let mut switch = None;
        if let Some(id) = self.switch_to
            && d.variants.iter().any(|v| v.id == id)
        {
            self.switch_to = None;
            switch = Some(id);
        }

        let Some(current) = d.variants.iter().find(|v| v.id == editing) else { return switch };
        let shown = d.primary_variant_id == Some(editing);
        let wait = "Apply or discard your changes first";
        let can = !self.busy && self.naming.is_none();

        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Version").color(P.faint));
            let mut chosen = None;
            ui.add_enabled_ui(can && !changed && d.variants.len() > 1, |ui| {
                egui::ComboBox::from_id_salt(("track-version", d.id))
                    .selected_text(name(current))
                    .show_ui(ui, |ui| {
                        for v in &d.variants {
                            let text = if d.primary_variant_id == Some(v.id) { format!("{}  · shown", name(v)) } else { name(v).to_string() };
                            if ui.selectable_label(v.id == editing, text).clicked() && v.id != editing {
                                chosen = Some(v.id);
                            }
                        }
                    })
                    .response
                    .on_disabled_hover_text(if changed { wait } else { "The track has only this one" });
            });
            if let Some(id) = chosen {
                switch = Some(id);
            }
            if shown {
                ui.label(RichText::new("· shown").color(P.accent));
            } else {
                ui.label(RichText::new("· not shown: applying changes only this version").color(P.muted));
                let button = ui.add_enabled(can && !changed, egui::Button::new("Show this one").small());
                if button.on_hover_text("The track takes this version's title, artists and the rest").on_disabled_hover_text(wait).clicked() {
                    self.send(shell, reqwest::Method::PUT, format!("/api/tracks/{}/primary-variant", d.id), json!({ "variant_id": editing }), Done::Shown);
                }
            }
            if ui.add_enabled(can, egui::Button::new("Rename…").small()).clicked() {
                self.naming = Some(Naming::Rename(editing, current.label.clone()));
                self.error = None;
                shell.ctx.memory_mut(|m| m.request_focus(egui::Id::new(NAME_ID)));
            }
            let button = ui.add_enabled(can && !changed, egui::Button::new("New version…").small());
            if button
                .on_hover_text("A copy of this one, to change on its own (an old title, a leak's name…)")
                .on_disabled_hover_text(wait)
                .clicked()
            {
                self.naming = Some(Naming::New(String::new()));
                self.error = None;
                shell.ctx.memory_mut(|m| m.request_focus(egui::Id::new(NAME_ID)));
            }
            if self.busy {
                ui.label(RichText::new("…").color(P.muted));
            }
        });

        // Naming a new version, or renaming this one: Enter does it.
        let mut done = None;
        if let Some(naming) = &mut self.naming {
            let (text, hint, action) = match naming {
                Naming::New(text) => (text, "The new version's name: “Leak title”, “Single edit”…", "Make it"),
                Naming::Rename(_, text) => (text, "Its name", "Rename"),
            };
            ui.horizontal(|ui| {
                let field = ui.add(egui::TextEdit::singleline(text).id(egui::Id::new(NAME_ID)).hint_text(hint).desired_width(320.0));
                let entered = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let ok = !text.trim().is_empty();
                if (ui.add_enabled(ok, egui::Button::new(action)).clicked() || entered) && ok {
                    done = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    done = Some(false);
                }
            });
        }
        match (done, self.naming.take()) {
            (Some(true), Some(Naming::New(label))) => {
                self.create(shell, d.id, json!({ "label": label.trim(), "copy_from": editing }));
            }
            (Some(true), Some(Naming::Rename(id, label))) => {
                self.send(shell, reqwest::Method::PATCH, format!("/api/tracks/{}/variants/{id}", d.id), json!({ "label": label.trim() }), Done::Renamed);
            }
            (Some(_), _) => {}
            (None, naming) => self.naming = naming,
        }
        if let Some(err) = &self.error {
            ui.label(RichText::new(err.as_str()).color(P.danger));
        }
        ui.add_space(4.0);
        switch
    }

    /// A new version named `label`, a copy of `from` (the commandline's `version:`): the
    /// editor edits it once it's there.
    pub fn make(&mut self, shell: &Shell, track: Uuid, label: &str, from: Uuid) {
        self.create(shell, track, json!({ "label": label.trim(), "copy_from": from }));
    }

    /// `variant` renamed `label` (the commandline's `version:… as:…`).
    pub fn rename(&mut self, shell: &Shell, track: Uuid, variant: Uuid, label: &str) {
        self.send(shell, reqwest::Method::PATCH, format!("/api/tracks/{track}/variants/{variant}"), json!({ "label": label.trim() }), Done::Renamed);
    }

    /// `variant` made the shown one (the commandline's "Show this version").
    pub fn make_shown(&mut self, shell: &Shell, track: Uuid, variant: Uuid) {
        self.send(shell, reqwest::Method::PUT, format!("/api/tracks/{track}/primary-variant"), json!({ "variant_id": variant }), Done::Shown);
    }

    /// Sends a request answered without a body; `done` says what it did.
    fn send(&mut self, shell: &Shell, method: reqwest::Method, path: String, body: Value, done: Done) {
        self.busy = true;
        self.answer.send_with(shell, |api| async move { api.send(method, &path, Some(body)).await.map(|()| done).map_err(|err| err.message) });
    }

    /// Makes a new version (`body`: its label, and the one it's a copy of).
    fn create(&mut self, shell: &Shell, track: Uuid, body: Value) {
        #[derive(serde::Deserialize)]
        struct Created {
            variant_id: Uuid,
        }
        self.busy = true;
        self.answer.send_with(shell, |api| async move {
            api.send_json::<Created>(reqwest::Method::POST, &format!("/api/tracks/{track}/variants"), Some(body))
                .await
                .map(|c| Done::Created(c.variant_id))
                .map_err(|err| err.message)
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::api::Api;
    use crate::details::TrackDetail;
    use crate::ui::edit::editor::model::Base;
    use uuid::Uuid;

    /// A track with two versions, the first shown.
    fn two_versions() -> TrackDetail {
        let (shown, other) = (Uuid::from_u128(2), Uuid::from_u128(4));
        serde_json::from_value(serde_json::json!({
            "id": Uuid::from_u128(1), "title": "Shown", "artist_display": "", "status": "released",
            "primary_variant_id": shown, "external_revision_id": Uuid::from_u128(11),
            "file": { "id": Uuid::from_u128(3), "sha256": "", "uploaded_filename": "", "size_bytes": 0, "uploaded_at": "" },
            "variants": [
                { "id": shown, "label": "File tags", "revision_id": Uuid::from_u128(10), "data": { "title": "Shown" } },
                { "id": other, "label": "Leak title", "revision_id": Uuid::from_u128(20), "data": { "title": "Leaked" } },
            ],
        }))
        .unwrap()
    }

    #[test]
    fn the_editor_starts_from_the_version_chosen() {
        let d = two_versions();
        let shown = Base::for_variant(&d, None).unwrap();
        assert_eq!((shown.variant, shown.variant_base, shown.meta.title.as_str()), (Uuid::from_u128(2), Uuid::from_u128(10), "Shown"));
        let other = Base::for_variant(&d, Some(Uuid::from_u128(4))).unwrap();
        assert_eq!((other.variant, other.variant_base, other.meta.title.as_str()), (Uuid::from_u128(4), Uuid::from_u128(20), "Leaked"));
        // A version the details don't have: the shown one.
        assert_eq!(Base::for_variant(&d, Some(Uuid::from_u128(99))).unwrap().variant, Uuid::from_u128(2));
    }

    /// Against a running server (a throwaway database: it adds a version to the track and
    /// shows it, then shows the first one again):
    /// `ACETATE_TEST_SERVER=http://127.0.0.1:3002 ACETATE_TEST_TOKEN=… ACETATE_TEST_TRACK=<id>
    /// cargo test versions -- --ignored`
    #[test]
    #[ignore]
    fn a_version_made_renamed_and_shown() {
        let vars = ["ACETATE_TEST_SERVER", "ACETATE_TEST_TOKEN", "ACETATE_TEST_TRACK"].map(std::env::var);
        let [Ok(server), Ok(token), Ok(track)] = vars else { return };
        let dir = std::env::temp_dir().join(format!("acetate-versions-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), serde_json::json!({"server": server, "token": token, "username": "test"}).to_string()).unwrap();
        let api = Api::new(dir);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (api, track) = (&api, track.as_str());
            let detail = || async move { api.send_json::<TrackDetail>(reqwest::Method::GET, &format!("/api/tracks/{track}"), None).await.unwrap() };
            let before = detail().await;
            let first = before.primary_variant_id.unwrap();

            // Made as the bar makes it: a copy of the one edited.
            #[derive(serde::Deserialize)]
            struct Created {
                variant_id: Uuid,
            }
            let body = serde_json::json!({ "label": "Test copy", "copy_from": first });
            let made: Created = api.send_json(reqwest::Method::POST, &format!("/api/tracks/{track}/variants"), Some(body)).await.unwrap();
            let d = detail().await;
            let copy = d.variants.iter().find(|v| v.id == made.variant_id).unwrap();
            let original = d.variants.iter().find(|v| v.id == first).unwrap();
            assert_eq!((copy.label.as_str(), &copy.data["title"]), ("Test copy", &original.data["title"]));
            assert_eq!(d.primary_variant_id, Some(first), "a new version isn't shown by itself");

            // Renamed and shown (answered without a body).
            let path = format!("/api/tracks/{track}/variants/{}", made.variant_id);
            api.send(reqwest::Method::PATCH, &path, Some(serde_json::json!({ "label": "Test copy, renamed" }))).await.unwrap();
            let primary = format!("/api/tracks/{track}/primary-variant");
            api.send(reqwest::Method::PUT, &primary, Some(serde_json::json!({ "variant_id": made.variant_id }))).await.unwrap();
            let d = detail().await;
            assert_eq!(d.primary_variant_id, Some(made.variant_id));
            assert_eq!(d.variants.iter().find(|v| v.id == made.variant_id).unwrap().label, "Test copy, renamed");

            // An empty name is refused, in words.
            let refused = api.send(reqwest::Method::PATCH, &path, Some(serde_json::json!({ "label": " " }))).await.unwrap_err();
            assert!(!refused.message.is_empty());

            api.send(reqwest::Method::PUT, &primary, Some(serde_json::json!({ "variant_id": first }))).await.unwrap();
            assert_eq!(detail().await.primary_variant_id, Some(first));
        });
    }
}
