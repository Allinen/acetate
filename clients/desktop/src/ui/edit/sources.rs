//! Editing where an album, artist or grouping is from (its sources), in edit mode, with the
//! `edit_sources` permission (a track's are changed in its editor: `source:`):
//!
//! * **Add** one: the platform first, then its fields, as the server describes them
//!   (`GET /api/source-shapes`: whole numbers, text, or lists of text, one per line; the
//!   field with the platform's id is marked). Pasting a link fills in the platform and
//!   its id where the link says them (SoundCloud, YouTube, NetEase); any other link makes
//!   a custom source with it.
//! * **Correct** one: a new snapshot of the same source, the form filled with its newest
//!   one. Its id stays: another id is another source (add that one, and put this one in
//!   the trash).
//! * **Trash** one (asks first). A source a file came from can't go (the server says
//!   which file); adding the same data again takes one back out of the trash.
//!
//! A track's sources are changed in its editor (`source:`, `origin:`); here only the
//! changes of where its file came from (see `OriginChanges`).


use eframe::egui::{self, RichText};
use serde_json::Value;
use uuid::Uuid;

use crate::account::SourceShape;
use crate::details::{OriginChange, Source};
use crate::ui::task::Task;
use crate::ui::theme::{font, semibold, PALETTE as P};
use crate::ui::Shell;

pub use acetate_core::editor::source_line::*;

/// Whose sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Album(Uuid),
    Artist(Uuid),
    /// A grouping (a collective's accounts: shaped like an artist's).
    Grouping(Uuid),
}

impl Owner {
    fn kind(self) -> &'static str {
        match self {
            Owner::Album(_) => "album",
            Owner::Artist(_) | Owner::Grouping(_) => "artist",
        }
    }

    fn path(self) -> String {
        match self {
            Owner::Album(id) => format!("/api/albums/{id}/sources"),
            Owner::Artist(id) => format!("/api/artists/{id}/sources"),
            Owner::Grouping(id) => format!("/api/groupings/{id}/sources"),
        }
    }

    /// Its details again (the sources changed).
    fn refresh(self, shell: &mut Shell) {
        match self {
            Owner::Album(id) => shell.album_details.refresh(id),
            Owner::Artist(id) => shell.artist_details.refresh(id),
            Owner::Grouping(id) => shell.grouping_details.refresh(id),
        }
    }
}

/// A source being added or corrected.
pub struct Form {
    /// Correcting this source (its platform and id stay).
    pub correcting: Option<Uuid>,
    pub platform: String,
    /// Each field of the platform's shape, as typed (lists: one item per line).
    values: Vec<(String, String)>,
    link: String,
}

#[derive(Default)]
pub struct SourcesEditor {
    owner: Option<Owner>,
    form: Option<Form>,
    /// "Trash…" was clicked on this source: asking to be sure.
    asked_trash: Option<Uuid>,
    busy: bool,
    answer: Task<Result<String, String>>,
    /// What the last save or trash said (Ok: done, in words).
    result: Option<Result<String, String>>,
}

impl SourcesEditor {
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, owner: Owner, sources: &[Source]) {
        if self.owner != Some(owner) {
            *self = SourcesEditor { owner: Some(owner), ..Default::default() };
        }
        if let Some(result) = self.answer.poll() {
            self.busy = false;
            if result.is_ok() {
                self.form = None;
                owner.refresh(shell);
            }
            self.result = Some(result);
        }
        if !shell.account.can("edit_sources") {
            ui.label(RichText::new("Changing sources needs the edit_sources permission.").color(P.faint).small());
            return;
        }
        let shapes: Vec<SourceShape> = shell
            .account
            .vocab
            .as_ref()
            .map(|v| v.source_shapes.iter().filter(|s| s.kind == owner.kind()).cloned().collect())
            .unwrap_or_default();

        // Numbered: `-source:#2` on a page's commandline puts the second in the trash.
        for (n, s) in sources.iter().enumerate() {
            let id = s.external_id.as_deref().map(|i| format!(" · {i}")).unwrap_or_default();
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("#{}", n + 1)).color(P.faint));
                ui.label(RichText::new(format!("{}{id}", s.platform)).font(font(13.5, semibold())));
                let editing = self.form.as_ref().is_some_and(|f| f.correcting == Some(s.id));
                if !editing && !self.busy && ui.small_button("Correct…").clicked() {
                    self.form = Some(Form::correcting(s, &shapes));
                    self.result = None;
                }
                if self.asked_trash != Some(s.id) && !self.busy && ui.small_button("Trash…").clicked() {
                    self.asked_trash = Some(s.id);
                    self.result = None;
                }
            });
            if self.asked_trash == Some(s.id) {
                ui.label(RichText::new("It goes to the trash, with its snapshots (adding the same data again takes it back out).").color(P.text));
                ui.horizontal(|ui| {
                    if ui.button("Move it to the trash").clicked() {
                        self.asked_trash = None;
                        self.send(shell, reqwest::Method::DELETE, format!("/api/sources/{}", s.id), None, "Moved to the trash.");
                    }
                    if ui.button("Cancel").clicked() {
                        self.asked_trash = None;
                    }
                });
            }
            if self.form.as_ref().is_some_and(|f| f.correcting == Some(s.id)) {
                self.form_ui(ui, shell, owner, &shapes);
            }
        }
        if sources.is_empty() {
            ui.label(RichText::new("No sources").color(P.muted));
        }

        if self.form.as_ref().is_some_and(|f| f.correcting.is_none()) {
            ui.add_space(6.0);
            self.form_ui(ui, shell, owner, &shapes);
        } else if !self.busy && ui.button("Add a source…").clicked() {
            self.form = Some(Form { correcting: None, platform: String::new(), values: Vec::new(), link: String::new() });
            self.result = None;
        }
        if self.busy {
            ui.label(RichText::new("Saving…").color(P.muted));
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
    }

    /// The form being filled in: a link to paste (adding), the platform, its fields.
    fn form_ui(&mut self, ui: &mut egui::Ui, shell: &Shell, owner: Owner, shapes: &[SourceShape]) {
        let Some(form) = self.form.as_mut() else { return };
        let mut save = false;
        let mut cancel = false;
        egui::Frame::NONE.fill(P.surface).corner_radius(8).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            if form.correcting.is_none() {
                let pasted = ui
                    .add(egui::TextEdit::singleline(&mut form.link).hint_text("Paste a link (SoundCloud, YouTube, NetEase, or any other)").desired_width(f32::INFINITY))
                    .changed();
                if pasted && let Some((platform, values)) = from_link(&form.link, owner.kind()) {
                    form.set_platform(&platform, shapes);
                    for (name, value) in values {
                        if let Some(field) = form.values.iter_mut().find(|(n, _)| *n == name) {
                            field.1 = value;
                        }
                    }
                }
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Platform").color(P.faint));
                    let mut chosen = None;
                    egui::ComboBox::from_id_salt("source-platform")
                        .selected_text(if form.platform.is_empty() { "choose…" } else { form.platform.as_str() })
                        .show_ui(ui, |ui| {
                            for shape in shapes {
                                if ui.selectable_label(form.platform == shape.platform, &shape.platform).clicked() {
                                    chosen = Some(shape.platform.clone());
                                }
                            }
                        });
                    if let Some(platform) = chosen {
                        form.set_platform(&platform, shapes);
                    }
                });
            } else {
                ui.label(RichText::new(format!("Correcting it: a new snapshot of this {} source, the old ones kept.", form.platform)).color(P.muted));
            }
            let Some(shape) = shapes.iter().find(|s| s.platform == form.platform) else { return };
            form.fields(ui, shape);
            ui.horizontal(|ui| {
                save = ui.button(if form.correcting.is_some() { "Save the correction" } else { "Add it" }).clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        if cancel {
            self.form = None;
        } else if save {
            let Some(form) = self.form.as_ref() else { return };
            let Some(shape) = shapes.iter().find(|s| s.platform == form.platform) else {
                self.result = Some(Err("Choose a platform first.".into()));
                return;
            };
            match form.data(shape) {
                Ok(data) => {
                    let body = serde_json::json!({ "platform": form.platform, "source_id": form.correcting, "data": data });
                    let done = if form.correcting.is_some() { "Corrected." } else { "Added." };
                    self.send(shell, reqwest::Method::POST, owner.path(), Some(body), done);
                }
                Err(err) => self.result = Some(Err(err)),
            }
        }
    }

    fn send(&mut self, shell: &Shell, method: reqwest::Method, path: String, body: Option<Value>, done: &'static str) {
        self.busy = true;
        self.result = None;
        self.answer.send_with(shell, |api| async move { api.send_json::<Value>(method, &path, body).await.map(|_| done.to_string()).map_err(|err| err.message) });
    }
}

impl Form {
    /// Filled with the source's newest snapshot.
    fn correcting(s: &Source, shapes: &[SourceShape]) -> Form {
        let data = s.snapshots.first().map(|snapshot| snapshot.data.clone()).unwrap_or_default();
        Form::for_data(s.id, &s.platform, &data, shapes)
    }

    /// Correcting source `id`, filled with `data`.
    pub fn for_data(id: Uuid, platform: &str, data: &Value, shapes: &[SourceShape]) -> Form {
        let mut form = Form { correcting: Some(id), platform: String::new(), values: Vec::new(), link: String::new() };
        form.set_platform(platform, shapes);
        for (name, text) in form.values.iter_mut() {
            *text = match data.get(name.as_str()) {
                Some(Value::String(v)) => v.clone(),
                Some(Value::Array(items)) => items.iter().map(|i| i.as_str().map(str::to_string).unwrap_or_else(|| i.to_string())).collect::<Vec<_>>().join("\n"),
                Some(Value::Null) | None => String::new(),
                Some(other) => other.to_string(),
            };
        }
        form
    }

    /// Another platform: its fields, empty (what was typed in a field of the same name
    /// stays).
    fn set_platform(&mut self, platform: &str, shapes: &[SourceShape]) {
        let Some(shape) = shapes.iter().find(|s| s.platform == platform) else { return };
        let old = std::mem::take(&mut self.values);
        self.platform = platform.to_string();
        self.values = shape
            .fields
            .iter()
            .map(|f| (f.name.clone(), old.iter().find(|(n, _)| *n == f.name).map(|(_, v)| v.clone()).unwrap_or_default()))
            .collect();
    }

    /// The data as the server takes it: only the fields filled in, as their type says.
    pub fn data(&self, shape: &SourceShape) -> Result<Value, String> {
        data_from(shape, &self.values)
    }

    /// The platform's fields, to fill in (a corrected source's id can't change).
    pub fn fields(&mut self, ui: &mut egui::Ui, shape: &SourceShape) {
        let correcting = self.correcting.is_some();
        egui::Grid::new(("source-fields", self.correcting)).num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            for (name, text) in self.values.iter_mut() {
                let Some(field) = shape.fields.iter().find(|f| f.name == *name) else { continue };
                let is_id = shape.id_field.as_deref() == Some(name.as_str());
                let mut label = name.replace('_', " ");
                if is_id {
                    label.push_str(" (its id)");
                }
                if field.required {
                    label.push_str(" *");
                }
                ui.label(RichText::new(label).color(if is_id { P.text } else { P.faint }));
                let hint = match field.kind.as_str() {
                    "int" => "a number",
                    "strs" => "one per line",
                    _ => "",
                };
                // A corrected source keeps its id: another id is another source.
                let locked = is_id && correcting;
                let edit = if field.kind == "strs" {
                    egui::TextEdit::multiline(text).desired_rows(2)
                } else {
                    egui::TextEdit::singleline(text)
                };
                let response = ui.add_enabled(!locked, edit.hint_text(hint).desired_width(360.0));
                if locked {
                    response.on_disabled_hover_text("Another id is another source: add that one, and put this one in the trash.");
                }
                ui.end_row();
            }
        });
    }
}

/// The changes of where a track's file came from (each with who, when and why; the newest
/// can be undone, with `edit_sources`). Which source it came from is chosen in the track's
/// editor (`origin:#N`).
#[derive(Default)]
pub struct OriginChanges {
    busy: bool,
    answer: Task<Result<(), String>>,
    error: Option<String>,
}

impl OriginChanges {
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, d: &crate::details::TrackDetail) {
        if let Some(result) = self.answer.poll() {
            self.busy = false;
            match result {
                Ok(()) => {
                    self.error = None;
                    shell.track_details.refresh(d.id);
                }
                Err(err) => self.error = Some(err),
            }
        }
        let can = shell.account.can("edit_sources") && !self.busy;
        let mut chosen = None;
        let changes: &[OriginChange] = d.origin_changes.as_deref().unwrap_or_default();
        for (n, c) in changes.iter().enumerate() {
            let named = |o: &Option<crate::details::OriginRef>| match o {
                Some(o) => format!("{}{}", o.platform, o.external_id.as_deref().map(|i| format!(" · {i}")).unwrap_or_default()),
                None => "(purged since)".into(),
            };
            let by = c.changed_by.as_deref().map(|b| format!(" by {b}")).unwrap_or_default();
            let why = Some(c.reason.trim()).filter(|r| !r.is_empty()).map(|r| format!(" (“{r}”)")).unwrap_or_default();
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("{} → {} · {}{by}{why}", named(&c.old), named(&c.new), c.changed_at.get(..10).unwrap_or(&c.changed_at)))
                        .color(P.muted)
                        .small(),
                );
                // The newest, while the file still comes from where it put it: back.
                let undoable = n == 0 && c.new.as_ref().map(|o| o.snapshot_id) == d.file.origin.as_ref().map(|o| o.snapshot_id);
                if undoable
                    && can
                    && let Some(old) = &c.old
                    && ui.small_button("Undo").on_hover_text("From where it was before again (a change of its own)").clicked()
                {
                    chosen = Some(serde_json::json!({ "snapshot_id": old.snapshot_id, "reason": "undo" }));
                }
            });
        }
        if let Some(body) = chosen {
            self.busy = true;
            let file = d.file.id;
            self.answer.send_with(shell, |api| async move { api.send_json::<Value>(reqwest::Method::PUT, &format!("/api/files/{file}/origin"), Some(body)).await.map(|_| ()).map_err(|err| err.message) });
        }
        if let Some(err) = &self.error {
            ui.label(RichText::new(err.as_str()).color(P.danger));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(link: &str, kind: &str) -> Option<(String, Vec<(String, String)>)> {
        from_link(link, kind)
    }

    #[test]
    fn links_fill_in_the_platform_and_id() {
        let one = |p: &str, f: &str, v: &str| Some((p.to_string(), vec![(f.to_string(), v.to_string())]));
        assert_eq!(filled("https://soundcloud.com/qlvarn/song-name?in=x", "track"), one("soundcloud", "song_permalinks", "qlvarn/song-name"));
        assert_eq!(filled("https://soundcloud.com/qlvarn/sets/tape", "album"), one("soundcloud", "album_permalinks", "qlvarn/sets/tape"));
        assert_eq!(filled("https://m.soundcloud.com/qlvarn", "artist"), one("soundcloud", "artist_permalinks", "qlvarn"));
        assert_eq!(filled("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=1", "track"), one("youtube", "song_id", "dQw4w9WgXcQ"));
        assert_eq!(filled("https://youtu.be/dQw4w9WgXcQ", "track"), one("youtube", "song_id", "dQw4w9WgXcQ"));
        assert_eq!(filled("https://www.youtube.com/@someone", "artist"), one("youtube", "channel", "someone"));
        assert_eq!(filled("https://music.163.com/#/song?id=123", "track"), one("netease", "song_id", "123"));
        assert_eq!(filled("https://music.163.com/album?id=9", "album"), one("netease", "album_id", "9"));
        // Another site: a custom source.
        let custom = filled("https://drive.google.com/file/d/x", "track").unwrap();
        assert_eq!(custom.0, "custom");
        assert_eq!(custom.1[0], ("label".to_string(), "drive.google.com".to_string()));
        // Not a link, or a link that isn't one for this kind.
        assert_eq!(filled("qlvarn/song", "track"), None);
        assert_eq!(filled("https://music.163.com/#/song?id=1", "album"), None);
    }
}

/// Against a running server (ignored by default): `ACETATE_TEST_SERVER`,
/// `ACETATE_TEST_TOKEN`, `ACETATE_TEST_TRACK` (it gets a NetEase source, its file comes
/// from there and back, and the source goes to the trash: a throwaway database).
#[cfg(test)]
mod against_a_server {
    use super::*;
    use crate::api::Api;
    use crate::details::TrackDetail;

    #[test]
    #[ignore]
    fn a_source_added_corrected_made_the_origin_and_trashed() {
        let vars = ["ACETATE_TEST_SERVER", "ACETATE_TEST_TOKEN", "ACETATE_TEST_TRACK"].map(std::env::var);
        let [Ok(server), Ok(token), Ok(track)] = vars else { return };
        let dir = std::env::temp_dir().join(format!("acetate-sources-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), serde_json::json!({"server": server, "token": token, "username": "test"}).to_string()).unwrap();
        let api = Api::new(dir);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (api, track) = (&api, track.as_str());
            let detail = || async move { api.send_json::<TrackDetail>(reqwest::Method::GET, &format!("/api/tracks/{track}"), None).await.unwrap() };
            let shapes: Vec<SourceShape> = api.send_json(reqwest::Method::GET, "/api/source-shapes", None).await.unwrap();
            let shape = shapes.iter().find(|s| s.platform == "netease" && s.kind == "track").unwrap();

            // Added, from a pasted link, as the form builds it.
            let song = 900_000 + std::process::id() as i64;
            let (platform, values) = from_link(&format!("https://music.163.com/#/song?id={song}"), "track").unwrap();
            let mut form = Form { correcting: None, platform: String::new(), values: Vec::new(), link: String::new() };
            form.set_platform(&platform, &shapes);
            form.values = form.values.into_iter().map(|(n, v)| (n.clone(), values.iter().find(|(f, _)| *f == n).map(|(_, v)| v.clone()).unwrap_or(v))).collect();
            let body = serde_json::json!({ "platform": form.platform, "source_id": null, "data": form.data(shape).unwrap() });
            api.send_json::<Value>(reqwest::Method::POST, &format!("/api/tracks/{track}/sources"), Some(body)).await.unwrap();
            let d = detail().await;
            let source = d.sources.iter().find(|s| s.external_id.as_deref() == Some(&song.to_string())).expect("added").clone();

            // Corrected: the same source, a new snapshot.
            let mut form = Form::correcting(&source, &shapes);
            form.values.iter_mut().find(|(n, _)| n == "song_index").unwrap().1 = "3".into();
            let body = serde_json::json!({ "platform": form.platform, "source_id": form.correcting, "data": form.data(shape).unwrap() });
            api.send_json::<Value>(reqwest::Method::POST, &format!("/api/tracks/{track}/sources"), Some(body)).await.unwrap();
            let d = detail().await;
            let corrected = d.sources.iter().find(|s| s.id == source.id).unwrap();
            assert_eq!(corrected.snapshots.len(), 2);
            assert_eq!(corrected.snapshots[0].data["song_index"], 3);

            // The file comes from there; it can't go to the trash then; back; trashed.
            let before = d.file.origin.clone().unwrap();
            let origin = |body: Value| async move { api.send_json::<Value>(reqwest::Method::PUT, &format!("/api/files/{}/origin", d.file.id), Some(body)).await };
            origin(serde_json::json!({ "source_id": source.id, "reason": "sources test" })).await.unwrap();
            let d2 = detail().await;
            assert_eq!(d2.file.origin.as_ref().map(|o| o.source_id), Some(source.id));
            let change = &d2.origin_changes.as_ref().unwrap()[0];
            assert_eq!((change.reason.as_str(), change.old.as_ref().map(|o| o.snapshot_id)), ("sources test", Some(before.snapshot_id)));
            let refused = api.send_json::<Value>(reqwest::Method::DELETE, &format!("/api/sources/{}", source.id), None).await.unwrap_err();
            assert!(refused.message.contains("came from"), "{}", refused.message);
            origin(serde_json::json!({ "snapshot_id": before.snapshot_id, "reason": "undo" })).await.unwrap();
            assert_eq!(detail().await.file.origin.map(|o| o.snapshot_id), Some(before.snapshot_id));
            api.send_json::<Value>(reqwest::Method::DELETE, &format!("/api/sources/{}", source.id), None).await.unwrap();
            assert!(!detail().await.sources.iter().any(|s| s.id == source.id));
        });
    }
}
