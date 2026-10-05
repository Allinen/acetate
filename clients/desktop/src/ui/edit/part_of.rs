//! "Part of": the groupings a track or album is in (collectives, eras, projects…), as
//! links. An album's, in edit mode: each can be taken out (×), and it can be put in
//! another: one of the groupings there are, or a new one (its kind and name, made on the
//! spot). Each change is sent at once and kept in the grouping's history. (A track's are
//! changed in its editor: `group:`, `-group:`.)

use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::catalog::Catalog;
use crate::ui::theme::{font, medium, PALETTE as P};
use crate::ui::views::grouping::KINDS;
use crate::ui::{widgets, Action, Page, Shell};

/// What's in the groupings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Member {
    Track(Uuid),
    Album(Uuid),
}

impl Member {
    fn body(self) -> Value {
        match self {
            Member::Track(id) => json!({ "track_ids": [id] }),
            Member::Album(id) => json!({ "album_ids": [id] }),
        }
    }
}

#[derive(Default)]
pub struct PartOf {
    busy: bool,
    answer: Arc<Mutex<Option<Result<Uuid, String>>>>,
    error: Option<String>,
    /// "New grouping…": its kind and name, as typed.
    new: Option<(String, String)>,
}

impl PartOf {
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog, member: Member, groupings: &[Uuid]) {
        if let Some(result) = self.answer.lock().unwrap().take() {
            self.busy = false;
            match result {
                Ok(grouping) => {
                    self.error = None;
                    self.new = None;
                    shell.grouping_details.refresh(grouping);
                    shell.act(Action::RefreshLibrary);
                }
                Err(err) => self.error = Some(err),
            }
        }
        let editing = shell.edit_mode && shell.account.can("edit_metadata") && matches!(member, Member::Album(_));
        if groupings.is_empty() && !editing {
            return;
        }
        let mut take_out = None;
        let mut put_in = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Part of").font(font(13.5, medium())).color(P.faint));
            for &id in groupings {
                let Some(g) = catalog.grouping(id) else { continue };
                if widgets::link(ui, &g.name, font(13.5, medium()), P.muted) {
                    shell.act(Action::Go(Page::Grouping(id)));
                }
                ui.label(RichText::new(format!("({})", g.kind)).font(font(12.5, medium())).color(P.faint));
                if editing && ui.add_enabled(!self.busy, egui::Button::new("×").small()).on_hover_text("Take it out of this").clicked() {
                    take_out = Some(id);
                }
            }
            if groupings.is_empty() {
                ui.label(RichText::new("nothing").color(P.faint));
            }
            if editing {
                let others: Vec<_> = catalog.groupings.iter().filter(|g| !groupings.contains(&g.id)).collect();
                ui.add_enabled_ui(!self.busy, |ui| {
                    egui::ComboBox::from_id_salt(("part-of-add", format!("{member:?}"))).selected_text("Add to…").show_ui(ui, |ui| {
                        for g in &others {
                            if ui.selectable_label(false, format!("{} · {}", g.name, g.kind)).clicked() {
                                put_in = Some(g.id);
                            }
                        }
                        if ui.selectable_label(false, "New grouping…").clicked() {
                            self.new = Some(("collective".into(), String::new()));
                        }
                    });
                });
            }
        });
        let mut cancel = false;
        let busy = self.busy;
        if let Some((kind, name)) = self.new.as_mut() {
            let mut make = false;
            ui.horizontal_wrapped(|ui| {
                for k in KINDS {
                    if widgets::chip(ui, k, kind == k) {
                        *kind = k.to_string();
                    }
                }
                ui.add(egui::TextEdit::singleline(kind).desired_width(100.0).hint_text("kind"));
                let field = ui.add(egui::TextEdit::singleline(name).desired_width(240.0).hint_text("its name"));
                make = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                make |= ui.add_enabled(!name.trim().is_empty() && !busy, egui::Button::new("Make it, and add this")).clicked();
                cancel = ui.button("Cancel").clicked();
            });
            if make && let Some((kind, name)) = self.new.clone().filter(|(_, n)| !n.trim().is_empty()) {
                self.send(shell, member, Change::New { kind, name });
            }
        }
        if cancel {
            self.new = None;
        }
        if let Some(id) = take_out {
            self.send(shell, member, Change::Remove(id));
        }
        if let Some(id) = put_in {
            self.send(shell, member, Change::Add(id));
        }
        if let Some(err) = &self.error {
            ui.label(RichText::new(err.as_str()).color(P.danger));
        }
    }

    fn send(&mut self, shell: &Shell, member: Member, change: Change) {
        self.busy = true;
        let (api, answer, ctx) = (shell.api.clone(), self.answer.clone(), shell.ctx.clone());
        shell.runtime.spawn(async move {
            let result = send(&api, member, &change).await;
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }
}

/// A change to the groupings something is in.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Add(Uuid),
    Remove(Uuid),
    New { kind: String, name: String },
}

/// Sends a change (the album's page sends them from its commandline too). Ok: the
/// grouping it changed.
pub async fn send(api: &crate::api::Api, member: Member, change: &Change) -> Result<Uuid, String> {
    #[derive(serde::Deserialize)]
    struct Made {
        id: Uuid,
    }
    let post = |path: String, body: Value| async move { api.send_json::<Value>(reqwest::Method::POST, &path, Some(body)).await.map_err(|e| e.message) };
    match change {
        Change::Add(id) => post(format!("/api/groupings/{id}/add"), member.body()).await.map(|_| *id),
        Change::Remove(id) => post(format!("/api/groupings/{id}/remove"), member.body()).await.map(|_| *id),
        Change::New { kind, name } => {
            let body = json!({ "kind": kind.trim(), "name": name.trim() });
            match api.send_json::<Made>(reqwest::Method::POST, "/api/groupings", Some(body)).await {
                Ok(made) => post(format!("/api/groupings/{}/add", made.id), member.body()).await.map(|_| made.id),
                Err(err) => Err(err.message),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Api;

    /// Against a running server (a throwaway database: it makes a grouping, puts the track
    /// in it and takes it out again; the grouping stays, as they do):
    /// `ACETATE_TEST_SERVER=http://127.0.0.1:3002 ACETATE_TEST_TOKEN=… ACETATE_TEST_TRACK=<id>
    /// cargo test part_of -- --ignored`
    #[test]
    #[ignore]
    fn a_track_put_in_a_grouping_shows_in_the_catalog() {
        let vars = ["ACETATE_TEST_SERVER", "ACETATE_TEST_TOKEN", "ACETATE_TEST_TRACK"].map(std::env::var);
        let [Ok(server), Ok(token), Ok(track)] = vars else { return };
        let track: Uuid = track.parse().unwrap();
        let dir = std::env::temp_dir().join(format!("acetate-part-of-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), json!({"server": server, "token": token, "username": "test"}).to_string()).unwrap();
        let api = Api::new(dir);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let catalog = || async {
                let crate::api::CatalogResponse::Changed { body, .. } = api.catalog(None, None).await.unwrap() else { panic!("a catalog") };
                Catalog::build(crate::catalog::model::parse(&body).unwrap())
            };
            #[derive(serde::Deserialize)]
            struct Made {
                id: Uuid,
            }
            let name = format!("Test crew {}", std::process::id());
            let made: Made = api.send_json(reqwest::Method::POST, "/api/groupings", Some(json!({ "kind": "collective", "name": name }))).await.unwrap();
            api.send_json::<Value>(reqwest::Method::POST, &format!("/api/groupings/{}/add", made.id), Some(Member::Track(track).body())).await.unwrap();

            // The catalog (format 2) has it, and the track in it.
            let c = catalog().await;
            let g = c.grouping(made.id).expect("in the catalog");
            assert_eq!((g.kind.as_str(), g.name.as_str(), g.track_count), ("collective", name.as_str(), 1));
            let i = c.track_index(track).unwrap();
            assert!(c.tracks[i as usize].groupings.contains(&made.id));
            assert_eq!(c.grouping_tracks(made.id), [i]);
            let q = crate::catalog::query::Query::parse(&format!("group:{name}"));
            assert!(q.matches_track(&c, i, ""));

            // Its page's details: the change is in its history.
            let d: crate::details::GroupingDetail = api.send_json(reqwest::Method::GET, &format!("/api/groupings/{}", made.id), None).await.unwrap();
            assert!(d.changes.iter().any(|c| c.action == "added" && c.track_id == Some(track)));

            api.send_json::<Value>(reqwest::Method::POST, &format!("/api/groupings/{}/remove", made.id), Some(Member::Track(track).body())).await.unwrap();
            assert!(!catalog().await.tracks[i as usize].groupings.contains(&made.id));
        });
    }
}
