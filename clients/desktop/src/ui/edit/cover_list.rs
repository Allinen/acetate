//! A track's or album's covers, in edit mode: in order, the first being the main one (the
//! one shown everywhere). Each can be made the main one, moved earlier or later, looked at
//! in full (a click: the original), or moved to the trash (asks first; the Trash page
//! brings it back). An album's cover can be put on all its tracks.
//!
//! New ones: copied from another track or album (picked in the palette, then one of its
//! covers chosen: the same image, stored once), or an image pasted (Ctrl+V) or dropped on
//! the window, uploaded at once.
//!
//! Every change is sent at once; the lists catch up (a track's cover shows everywhere).

use std::sync::{Arc, Mutex};

use eframe::egui::{self, Key, RichText, Sense, Vec2};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::details::Cover;
use crate::ui::edit::PickKind;
use crate::ui::theme::{font, semibold, PALETTE as P};
use crate::ui::{Action, Shell};

/// Whose covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Track(Uuid),
    Album(Uuid),
}

impl Owner {
    fn path(self) -> String {
        match self {
            Owner::Track(id) => format!("/api/tracks/{id}/covers"),
            Owner::Album(id) => format!("/api/albums/{id}/covers"),
        }
    }

    fn refresh(self, shell: &mut Shell) {
        match self {
            Owner::Track(id) => shell.track_details.refresh(id),
            Owner::Album(id) => shell.album_details.refresh(id),
        }
        // What a track or album shows comes from its covers: the lists catch up.
        shell.act(Action::RefreshLibrary);
    }
}

/// Where a copy comes from (picked in the palette).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum From {
    Track(Uuid),
    Album(Uuid),
}

#[derive(Default)]
pub struct CoverList {
    owner: Option<Owner>,
    busy: bool,
    answer: Arc<Mutex<Option<Result<String, String>>>>,
    /// What the last change said (Ok: done, in words).
    result: Option<Result<String, String>>,
    /// "Trash…" was clicked on this cover: asking to be sure.
    asked_trash: Option<Uuid>,
    /// The palette was asked for a track or album to copy from.
    picking: bool,
    /// Choosing one of its covers to copy.
    copy_from: Option<From>,
}

impl CoverList {
    /// Takes the palette's answer when it's for this (a track or album to copy from).
    /// Called before the page's forms are shown, which would take it otherwise.
    pub fn take_pick(&mut self, shell: &mut Shell) {
        if !self.picking {
            return;
        }
        let Some(picked) = shell.picker.take() else { return };
        self.picking = false;
        self.copy_from = match (picked.kind, picked.id) {
            (PickKind::Track, Some(id)) => Some(From::Track(id)),
            (PickKind::Album, Some(id)) => Some(From::Album(id)),
            _ => None,
        };
    }

    /// `paste`: a pasted or dropped image is uploaded as a new cover (where only one
    /// cover list is shown).
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, owner: Owner, covers: &[Cover], paste: bool) {
        if self.owner != Some(owner) {
            *self = CoverList { owner: Some(owner), ..Default::default() };
        }
        if let Some(result) = self.answer.lock().unwrap().take() {
            self.busy = false;
            if result.is_ok() {
                owner.refresh(shell);
            }
            self.result = Some(result);
        }
        let can = shell.account.can("edit_metadata") && !self.busy;

        // A pasted or dropped image: a new cover, now.
        if paste && can && !shell.palette_open && !shell.modal_open {
            match take_image(ui.ctx()) {
                Some(Ok((bytes, name))) => self.upload(shell, owner, bytes, name),
                Some(Err(err)) => self.result = Some(Err(err)),
                None => {}
            }
        }

        // ---- The covers, in order ----
        if covers.is_empty() {
            ui.label(RichText::new("No covers of its own.").color(P.muted));
        }
        let mut order: Option<Vec<Uuid>> = None;
        ui.horizontal_wrapped(|ui| {
            for (n, c) in covers.iter().enumerate() {
                ui.vertical(|ui| {
                    ui.set_width(132.0);
                    let (rect, response) = ui.allocate_exact_size(Vec2::splat(132.0), Sense::click());
                    shell.cover(ui, c.cover(), c.id, rect, 8, 0.1);
                    let size = match (c.width, c.height) {
                        (Some(w), Some(h)) => format!("{w}×{h} · {}", c.source),
                        _ => c.source.clone(),
                    };
                    if response.on_hover_cursor(egui::CursorIcon::ZoomIn).on_hover_text(format!("{size} · click: the original")).clicked()
                        && let Some(cover) = c.cover()
                    {
                        shell.open_original(ui.ctx(), cover);
                    }
                    ui.horizontal(|ui| {
                        if n == 0 {
                            ui.label(RichText::new("Main").font(font(12.5, semibold())).color(P.accent));
                        } else if ui.add_enabled(can, egui::Button::new("Main").small()).on_hover_text("Make it the main cover (shown everywhere)").clicked() {
                            order = Some(moved(covers, n, 0));
                        }
                        if n > 0 && ui.add_enabled(can, egui::Button::new("←").small()).on_hover_text("Earlier").clicked() {
                            order = Some(moved(covers, n, n - 1));
                        }
                        if n + 1 < covers.len() && ui.add_enabled(can, egui::Button::new("→").small()).on_hover_text("Later").clicked() {
                            order = Some(moved(covers, n, n + 1));
                        }
                        if self.asked_trash != Some(c.id) && ui.add_enabled(can, egui::Button::new("🗑").small()).on_hover_text("Move it to the trash…").clicked() {
                            self.asked_trash = Some(c.id);
                            self.result = None;
                        }
                    });
                    if let Owner::Album(album) = owner
                        && ui
                            .add_enabled(can, egui::Button::new("On all its tracks").small())
                            .on_hover_text("Adds this image to each of the album's tracks that hasn't got it (as main where a track has no cover)")
                            .clicked()
                    {
                        let path = format!("/api/albums/{album}/covers/apply-to-tracks");
                        self.send(shell, reqwest::Method::POST, path, Some(json!({ "cover_id": c.id })), "Put on its tracks.");
                    }
                });
            }
        });
        if let Some(id) = self.asked_trash {
            ui.label(RichText::new("It goes to the trash (the Trash page, next to Settings, brings it back). The image stays wherever else it's used.").color(P.text));
            ui.horizontal(|ui| {
                if ui.button("Move it to the trash").clicked() {
                    self.asked_trash = None;
                    self.send(shell, reqwest::Method::DELETE, format!("/api/covers/{id}"), None, "Moved to the trash.");
                }
                if ui.button("Cancel").clicked() {
                    self.asked_trash = None;
                }
            });
        }
        if let Some(ids) = order {
            let body = json!({ "cover_ids": ids });
            self.send(shell, reqwest::Method::PUT, format!("{}/order", owner.path()), Some(body), "Reordered.");
        }

        // ---- Adding ----
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            let except = match owner {
                Owner::Track(id) | Owner::Album(id) => Some(id),
            };
            if ui.add_enabled(can, egui::Button::new("Copy from a track…")).clicked() {
                self.picking = true;
                shell.act(Action::Pick { kind: PickKind::Track, except });
            }
            if ui.add_enabled(can, egui::Button::new("Copy from an album…")).clicked() {
                self.picking = true;
                shell.act(Action::Pick { kind: PickKind::Album, except });
            }
            if paste {
                ui.label(RichText::new("or paste an image (Ctrl+V), or drop one on the window").color(P.faint).small());
            }
        });
        if let Some(from) = self.copy_from {
            self.choose_copy(ui, shell, owner, from, can);
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

    /// The covers of the track or album picked to copy from: a click copies one.
    fn choose_copy(&mut self, ui: &mut egui::Ui, shell: &mut Shell, owner: Owner, from: From, can: bool) {
        let (name, covers) = match from {
            From::Track(id) => {
                let d = shell.track_details.get(id).value.clone();
                (d.as_ref().map(|d| d.title.clone()), d.map(|d| d.covers.clone()))
            }
            From::Album(id) => {
                let d = shell.album_details.get(id).value.clone();
                (d.as_ref().map(|d| d.title.clone()), d.map(|d| d.covers.clone()))
            }
        };
        egui::Frame::NONE.fill(P.surface).corner_radius(8).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.horizontal(|ui| {
                let title = name.map(|n| format!("Copy one of “{n}”'s covers")).unwrap_or_else(|| "Loading its covers…".into());
                ui.label(RichText::new(title).font(font(13.5, semibold())));
                if ui.small_button("Cancel").clicked() {
                    self.copy_from = None;
                }
            });
            let Some(covers) = covers else { return };
            if covers.is_empty() {
                ui.label(RichText::new("It has no covers of its own.").color(P.muted));
            }
            let mut chosen = None;
            ui.horizontal_wrapped(|ui| {
                for c in &covers {
                    let (rect, response) = ui.allocate_exact_size(Vec2::splat(96.0), Sense::click());
                    shell.cover(ui, c.cover(), c.id, rect, 6, 0.0);
                    if response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Copy this one").clicked() && can {
                        chosen = Some(c.id);
                    }
                }
            });
            if let Some(id) = chosen {
                self.copy_from = None;
                self.send(shell, reqwest::Method::POST, format!("{}/copy", owner.path()), Some(json!({ "cover_id": id })), "Copied.");
            }
        });
    }

    fn send(&mut self, shell: &Shell, method: reqwest::Method, path: String, body: Option<Value>, done: &'static str) {
        self.busy = true;
        self.result = None;
        let (api, answer, ctx) = (shell.api.clone(), self.answer.clone(), shell.ctx.clone());
        shell.runtime.spawn(async move {
            let result = api.send(method, &path, body).await.map(|()| done.to_string()).map_err(|err| err.message);
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }

    fn upload(&mut self, shell: &Shell, owner: Owner, bytes: Vec<u8>, name: String) {
        self.busy = true;
        self.result = None;
        let (api, answer, ctx, path) = (shell.api.clone(), self.answer.clone(), shell.ctx.clone(), owner.path());
        shell.runtime.spawn(async move {
            let result = api.upload::<Value>(&path, bytes, &name).await.map(|_| "Added.".to_string()).map_err(|err| err.message);
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }
}

/// The ids in order, with the one at `from` moved to `to`.
fn moved(covers: &[Cover], from: usize, to: usize) -> Vec<Uuid> {
    let mut ids: Vec<Uuid> = covers.iter().map(|c| c.id).collect();
    let id = ids.remove(from);
    ids.insert(to, id);
    ids
}

/// An image pasted (Ctrl+V: an image in the clipboard, not text) or dropped on the window
/// this frame: its bytes and a file name. Err: a dropped file that couldn't be read or
/// isn't an image.
pub fn take_image(ctx: &egui::Context) -> Option<Result<(Vec<u8>, String), String>> {
    let pasted = ctx.input(|i| {
        i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)) || matches!(e, egui::Event::Key { key: Key::V, pressed: true, modifiers, .. } if modifiers.command))
    });
    let mut found = None;
    if pasted {
        // Text in the clipboard is for a text field; only an image is taken here.
        if let Ok(image) = arboard::Clipboard::new().and_then(|mut c| c.get_image()) {
            let rgba = image::RgbaImage::from_raw(image.width as u32, image.height as u32, image.bytes.into_owned());
            let mut png = Vec::new();
            if let Some(rgba) = rgba
                && rgba.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).is_ok()
            {
                found = Some(Ok((png, "pasted.png".to_string())));
            }
        }
    }
    for file in ctx.input(|i| i.raw.dropped_files.clone()) {
        let name = file.path().file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        found = Some(match file.bytes() {
            Ok(bytes) if image::guess_format(&bytes).is_ok() => Ok((bytes, name)),
            Ok(_) => Err(format!("“{name}” isn't an image this can read (JPEG, PNG, WebP or GIF).")),
            Err(err) => Err(format!("Couldn't read “{name}”: {err}")),
        });
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_a_cover_keeps_the_others_in_order() {
        let covers: Vec<Cover> = (1..=4)
            .map(|n| serde_json::from_value(json!({ "id": Uuid::from_u128(n), "blob_sha256": "", "source": "" })).unwrap())
            .collect();
        let id = Uuid::from_u128;
        assert_eq!(moved(&covers, 2, 0), [id(3), id(1), id(2), id(4)], "made main");
        assert_eq!(moved(&covers, 1, 2), [id(1), id(3), id(2), id(4)], "later");
    }

    /// Against a running server (a throwaway database: it adds covers to two tracks,
    /// then moves them to the trash):
    /// `ACETATE_TEST_SERVER=http://127.0.0.1:3002 ACETATE_TEST_TOKEN=… ACETATE_TEST_TRACK=<id>
    /// ACETATE_TEST_OTHER_TRACK=<id> cargo test covers -- --ignored`
    #[test]
    #[ignore]
    fn a_cover_uploaded_copied_reordered_and_trashed() {
        use crate::api::Api;
        use crate::details::TrackDetail;
        let vars = ["ACETATE_TEST_SERVER", "ACETATE_TEST_TOKEN", "ACETATE_TEST_TRACK", "ACETATE_TEST_OTHER_TRACK"].map(std::env::var);
        let [Ok(server), Ok(token), Ok(track), Ok(other)] = vars else { return };
        let dir = std::env::temp_dir().join(format!("acetate-covers-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), json!({"server": server, "token": token, "username": "test"}).to_string()).unwrap();
        let api = Api::new(dir);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let api = &api;
            let detail = |id: String| async move { api.send_json::<TrackDetail>(reqwest::Method::GET, &format!("/api/tracks/{id}"), None).await.unwrap() };
            // A new image (unlike any stored: the process id in its pixels).
            let seed = std::process::id();
            let image = image::RgbImage::from_fn(64, 64, |x, y| image::Rgb([(seed % 251) as u8, x as u8, y as u8]));
            let mut png = Vec::new();
            image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();

            let owner = Owner::Track(track.parse().unwrap());
            let before = detail(track.clone()).await.covers.len();
            api.upload::<Value>(&owner.path(), png, "test.png").await.unwrap();
            let covers = detail(track.clone()).await.covers;
            assert_eq!(covers.len(), before + 1);
            let new = covers.last().unwrap().clone();

            // Made the main one, as "Main" sends it.
            let order = moved(&covers, covers.len() - 1, 0);
            api.send(reqwest::Method::PUT, &format!("{}/order", owner.path()), Some(json!({ "cover_ids": order }))).await.unwrap();
            assert_eq!(detail(track.clone()).await.covers[0].id, new.id);

            // Copied to the other track: the same image.
            let to = Owner::Track(other.parse().unwrap());
            api.send(reqwest::Method::POST, &format!("{}/copy", to.path()), Some(json!({ "cover_id": new.id }))).await.unwrap();
            let copy = detail(other.clone()).await.covers.into_iter().find(|c| c.blob_sha256 == new.blob_sha256).expect("copied");

            // Both to the trash.
            for id in [new.id, copy.id] {
                api.send(reqwest::Method::DELETE, &format!("/api/covers/{id}"), None).await.unwrap();
            }
            assert!(detail(track.clone()).await.covers.iter().all(|c| c.id != new.id));
            assert!(detail(other.clone()).await.covers.iter().all(|c| c.id != copy.id));
        });
    }
}
