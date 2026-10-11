//! An album's covers, in edit mode: in order, the first being the main one (the one shown
//! everywhere). Each can be made the main one, moved earlier or later, looked at in full
//! (a click: the original), or moved to the trash (the Trash page brings it back), or put
//! on all its tracks. (A track's are changed in its editor: `cover:`.)
//!
//! New ones: copied from another track or album (picked in the palette, then one of its
//! covers chosen: the same image, stored once), or an image pasted (Ctrl+V), chosen or
//! dropped on the window.
//!
//! Changes are a draft, as on the phone: nothing is sent until Save, which sends them all
//! (Discard drops them); the lists then catch up (a track's cover shows everywhere).

use std::sync::{Arc, Mutex};

use eframe::egui::{self, Key, RichText, Sense, Vec2};
use serde_json::json;
use uuid::Uuid;

use crate::details::Cover;
use crate::ui::edit::PickKind;
use crate::ui::theme::{font, semibold, PALETTE as P};
use crate::ui::{Action, Shell};

/// Whose covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Album(Uuid),
}

impl Owner {
    /// Its covers' address (`/order`, `/copy` after it; an image posted to it is a new one).
    pub fn path(self) -> String {
        match self {
            Owner::Album(id) => format!("/api/albums/{id}/covers"),
        }
    }

    fn refresh(self, shell: &mut Shell) {
        match self {
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

/// One cover in the list being edited.
struct Item {
    what: What,
    /// To the trash when saved.
    trash: bool,
    /// Put on all the album's tracks when saved.
    on_tracks: bool,
}

enum What {
    /// One the album has.
    Existing(Cover),
    /// An image to add (pasted, dropped or chosen), with a small copy to look at.
    Image { bytes: Vec<u8>, name: String, preview: Option<egui::TextureHandle> },
    /// A copy of another track's or album's cover (the same image, stored once).
    Copy(Cover),
}

#[derive(Default)]
pub struct CoverList {
    owner: Option<Owner>,
    /// The covers as edited, once anything is changed; none: the album's, unchanged. They
    /// are sent together by Save, and gone by Discard.
    draft: Option<Vec<Item>>,
    busy: bool,
    answer: Arc<Mutex<Option<Result<String, String>>>>,
    /// What the last save said (Ok: done, in words).
    result: Option<Result<String, String>>,
    /// The palette was asked for a track or album to copy from.
    picking: bool,
    /// Choosing one of its covers to copy.
    copy_from: Option<From>,
    /// An image chosen in the system's file dialog ("Choose an image…").
    chosen: std::sync::Arc<std::sync::Mutex<Vec<std::path::PathBuf>>>,
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

    /// The draft, started from the album's covers if there is none yet.
    fn draft<'a>(draft: &'a mut Option<Vec<Item>>, covers: &[Cover]) -> &'a mut Vec<Item> {
        draft.get_or_insert_with(|| covers.iter().map(|c| Item { what: What::Existing(c.clone()), trash: false, on_tracks: false }).collect())
    }

    fn add(&mut self, covers: &[Cover], what: What) {
        Self::draft(&mut self.draft, covers).push(Item { what, trash: false, on_tracks: false });
    }

    /// `paste`: a pasted or dropped image is added as a new cover (where only one cover
    /// list is shown). Nothing is sent until Save.
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, owner: Owner, covers: &[Cover], paste: bool) {
        if self.owner != Some(owner) {
            *self = CoverList { owner: Some(owner), ..Default::default() };
        }
        if let Some(result) = self.answer.lock().unwrap().take() {
            self.busy = false;
            // Saved (or not, and then it shows what the album has): the draft is done with.
            self.draft = None;
            owner.refresh(shell);
            self.result = Some(result);
        }
        let can = shell.account.can("edit_metadata") && !self.busy;

        // A pasted, dropped or chosen image: a new cover in the draft. (Chosen ones are this
        // list's; pasted and dropped ones go to the one list that takes them.)
        let chosen = std::mem::take(&mut *self.chosen.lock().unwrap());
        let taking = paste && !shell.palette_open && !shell.modal_open;
        if can && (taking || !chosen.is_empty()) {
            match take_image(ui.ctx(), taking, chosen) {
                Some(Ok((bytes, name))) => {
                    let preview = preview_of(ui.ctx(), &bytes);
                    self.add(covers, What::Image { bytes, name, preview });
                    self.result = None;
                }
                Some(Err(err)) => self.result = Some(Err(err)),
                None => {}
            }
        }

        // ---- The covers, in order ----
        let n_shown = self.draft.as_ref().map_or(covers.len(), Vec::len);
        if n_shown == 0 {
            ui.label(RichText::new("No covers of its own.").color(P.muted));
        }
        // What was asked this frame (applied after drawing).
        enum Ask {
            Main(usize),
            Move(usize, usize),
            Trash(usize),
            OnTracks(usize),
        }
        let mut ask = None;
        ui.horizontal_wrapped(|ui| {
            for n in 0..n_shown {
                // (Drawn from the draft, else from the album's own covers.)
                let (existing, item) = match &self.draft {
                    Some(items) => (None, Some(&items[n])),
                    None => (Some(&covers[n]), None),
                };
                let (trash, on_tracks) = item.map_or((false, false), |i| (i.trash, i.on_tracks));
                ui.vertical(|ui| {
                    ui.set_width(132.0);
                    let (rect, response) = ui.allocate_exact_size(Vec2::splat(132.0), Sense::click());
                    let mut note = String::new();
                    match (existing, item.map(|i| &i.what)) {
                        (Some(c), _) | (None, Some(What::Existing(c))) => {
                            shell.cover(ui, c.cover(), c.id, rect, 8, 0.1);
                            note = match (c.width, c.height) {
                                (Some(w), Some(h)) => format!("{w}×{h} · {}", c.source),
                                _ => c.source.clone(),
                            };
                        }
                        (None, Some(What::Copy(c))) => {
                            shell.cover(ui, c.cover(), c.id, rect, 8, 0.1);
                            note = "a copy, added when saved".into();
                        }
                        (None, Some(What::Image { preview, name, .. })) => {
                            match preview {
                                Some(texture) => {
                                    ui.painter().image(texture.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                                }
                                None => {
                                    ui.painter().rect_stroke(rect, 8, egui::Stroke::new(1.0, P.line), egui::StrokeKind::Inside);
                                }
                            }
                            note = format!("{name} · added when saved");
                        }
                        (None, None) => {}
                    }
                    if trash {
                        ui.painter().rect_filled(rect, 8, egui::Color32::from_black_alpha(150));
                        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "to the trash", font(13.0, semibold()), P.text);
                    }
                    let opens = match (existing, item.map(|i| &i.what)) {
                        (Some(c), _) | (None, Some(What::Existing(c))) | (None, Some(What::Copy(c))) => c.cover(),
                        _ => None,
                    };
                    if response.on_hover_cursor(egui::CursorIcon::ZoomIn).on_hover_text(format!("{note} · click: the original")).clicked()
                        && let Some(cover) = opens
                    {
                        shell.open_original(ui.ctx(), cover);
                    }
                    let last = n_shown;
                    ui.horizontal(|ui| {
                        if n == 0 {
                            ui.label(RichText::new("Main").font(font(12.5, semibold())).color(P.accent));
                        } else if ui.add_enabled(can && !trash, egui::Button::new("Main").small()).on_hover_text("Make it the main cover (shown everywhere)").clicked() {
                            ask = Some(Ask::Main(n));
                        }
                        if n > 0 && ui.add_enabled(can, egui::Button::new("←").small()).on_hover_text("Earlier").clicked() {
                            ask = Some(Ask::Move(n, n - 1));
                        }
                        if n + 1 < last && ui.add_enabled(can, egui::Button::new("→").small()).on_hover_text("Later").clicked() {
                            ask = Some(Ask::Move(n, n + 1));
                        }
                        let (label, tip) = if trash { ("↩", "Keep it") } else { ("🗑", "Move it to the trash (when saved; the Trash page brings it back)") };
                        if ui.add_enabled(can, egui::Button::new(label).small()).on_hover_text(tip).clicked() {
                            ask = Some(Ask::Trash(n));
                        }
                    });
                    let on = on_tracks && !trash;
                    if ui
                        .add_enabled(can && !trash, egui::Button::new(if on { "On all its tracks ✓" } else { "On all its tracks" }).small().selected(on))
                        .on_hover_text("When saved, adds this image to each of the album's tracks that hasn't got it (as main where a track has no cover)")
                        .clicked()
                    {
                        ask = Some(Ask::OnTracks(n));
                    }
                });
            }
        });
        if let Some(ask) = ask {
            self.result = None;
            let items = Self::draft(&mut self.draft, covers);
            match ask {
                Ask::Main(n) => {
                    let item = items.remove(n);
                    items.insert(0, item);
                }
                Ask::Move(from, to) => {
                    let item = items.remove(from);
                    items.insert(to, item);
                }
                Ask::Trash(n) => {
                    // One not added yet just goes; one the album has is marked.
                    if matches!(items[n].what, What::Existing(_)) {
                        items[n].trash = !items[n].trash;
                    } else {
                        items.remove(n);
                    }
                }
                Ask::OnTracks(n) => items[n].on_tracks = !items[n].on_tracks,
            }
        }

        // ---- Adding ----
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            let except = match owner {
                Owner::Album(id) => Some(id),
            };
            if ui.add_enabled(can, egui::Button::new("Copy from a track…")).clicked() {
                self.picking = true;
                shell.act(Action::Pick { kind: PickKind::Track, except });
            }
            if ui.add_enabled(can, egui::Button::new("Copy from an album…")).clicked() {
                self.picking = true;
                shell.act(Action::Pick { kind: PickKind::Album, except });
            }
            if ui.add_enabled(can, egui::Button::new("Choose an image…")).clicked() {
                crate::ui::files::choose(shell, "A cover", ("Images", crate::ui::files::IMAGES), false, &self.chosen);
            }
            if paste {
                ui.label(RichText::new("or paste an image or a copied image file (Ctrl+V), or drop one on the window (not on Wayland)").color(P.faint).small());
            }
        });
        if let Some(from) = self.copy_from {
            self.choose_copy(ui, shell, covers, from, can);
        }

        // ---- Save, or not ----
        if let Some(items) = &self.draft {
            let changes = changes(items, covers);
            if changes > 0 {
                // Esc is for the draft (Discard), not for leaving the page.
                shell.form_dirty = true;
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(can, egui::Button::new(format!("Save the covers ({changes} change{})", if changes == 1 { "" } else { "s" }))).clicked() {
                        self.save(shell, owner);
                    }
                    if ui.add_enabled(!self.busy, egui::Button::new("Discard")).clicked() {
                        self.draft = None;
                        self.result = None;
                    }
                });
            } else {
                self.draft = None;
            }
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

    /// The covers of the track or album picked to copy from: a click adds a copy to the
    /// draft.
    fn choose_copy(&mut self, ui: &mut egui::Ui, shell: &mut Shell, own: &[Cover], from: From, can: bool) {
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
                        chosen = Some(c.clone());
                    }
                }
            });
            if let Some(cover) = chosen {
                self.copy_from = None;
                self.result = None;
                self.add(own, What::Copy(cover));
            }
        });
    }

    /// Sends the draft: the new images and copies first (the server puts them last, in the
    /// order they came, and says their ids), then the trash, the order, and the tracks.
    fn save(&mut self, shell: &Shell, owner: Owner) {
        let Some(items) = self.draft.take() else { return };
        self.draft = None;
        self.busy = true;
        self.result = None;
        let (api, answer, ctx, path) = (shell.api.clone(), self.answer.clone(), shell.ctx.clone(), owner.path());
        let Owner::Album(album) = owner;
        shell.runtime.spawn(async move {
            let result = async {
                let mut ids: Vec<(Uuid, bool)> = Vec::new(); // (cover, on all its tracks)
                let mut trashed = Vec::new();
                for item in items {
                    match item.what {
                        What::Existing(c) if item.trash => trashed.push(c.id),
                        What::Existing(c) => ids.push((c.id, item.on_tracks)),
                        What::Image { bytes, name, .. } => {
                            let added: Added = api.upload(&path, bytes, &name).await.map_err(|e| e.message)?;
                            ids.push((added.id, item.on_tracks));
                        }
                        What::Copy(c) => {
                            let added: Added = api.send_json(reqwest::Method::POST, &format!("{path}/copy"), Some(json!({ "cover_id": c.id }))).await.map_err(|e| e.message)?;
                            ids.push((added.id, item.on_tracks));
                        }
                    }
                }
                for id in &trashed {
                    api.send(reqwest::Method::DELETE, &format!("/api/covers/{id}"), None).await.map_err(|e| e.message)?;
                }
                let order: Vec<Uuid> = ids.iter().map(|i| i.0).collect();
                api.send(reqwest::Method::PUT, &format!("{path}/order"), Some(json!({ "cover_ids": order }))).await.map_err(|e| e.message)?;
                for (id, _) in ids.iter().filter(|i| i.1) {
                    api.send(reqwest::Method::POST, &format!("/api/albums/{album}/covers/apply-to-tracks"), Some(json!({ "cover_id": id }))).await.map_err(|e| e.message)?;
                }
                Ok::<String, String>("Saved.".to_string())
            }
            .await;
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }
}

#[derive(serde::Deserialize)]
struct Added {
    id: Uuid,
}

/// How many things Save would do: covers added, trashed, put on the tracks, and the order
/// if it isn't the album's.
fn changes(items: &[Item], covers: &[Cover]) -> usize {
    let added = items.iter().filter(|i| !matches!(i.what, What::Existing(_))).count();
    let trashed = items.iter().filter(|i| i.trash).count();
    let on_tracks = items.iter().filter(|i| i.on_tracks && !i.trash).count();
    let kept: Vec<Uuid> = items.iter().filter(|i| !i.trash).filter_map(|i| if let What::Existing(c) = &i.what { Some(c.id) } else { None }).collect();
    let before: Vec<Uuid> = covers.iter().map(|c| c.id).filter(|id| !items.iter().any(|i| i.trash && matches!(&i.what, What::Existing(c) if c.id == *id))).collect();
    added + trashed + on_tracks + usize::from(kept != before)
}

/// A small copy of an image to look at in the list (none: it can't be read).
fn preview_of(ctx: &egui::Context, bytes: &[u8]) -> Option<egui::TextureHandle> {
    let image = image::load_from_memory(bytes).ok()?.thumbnail(264, 264).to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Some(ctx.load_texture("new-cover", egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()), egui::TextureOptions::LINEAR))
}

/// The ids in order, with the one at `from` moved to `to`.
#[cfg(test)]
fn moved(covers: &[Cover], from: usize, to: usize) -> Vec<Uuid> {
    let mut ids: Vec<Uuid> = covers.iter().map(|c| c.id).collect();
    let id = ids.remove(from);
    ids.insert(to, id);
    ids
}

/// The image in the clipboard (an image, or a copied image file), as for a new cover: its
/// bytes and a file name. None: there's none.
pub fn clipboard_image() -> Option<Result<(Vec<u8>, String), String>> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    if let Ok(image) = clipboard.get_image() {
        let rgba = image::RgbaImage::from_raw(image.width as u32, image.height as u32, image.bytes.into_owned())?;
        let mut png = Vec::new();
        return Some(rgba.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map(|()| (png, "pasted.png".to_string())).map_err(|err| err.to_string()));
    }
    let files = clipboard.get().file_list().ok()?;
    let path = files.into_iter().map(|p| std::path::PathBuf::from(p.to_string_lossy().trim_end_matches(['\r', '\n']))).find(|p| {
        crate::ui::files::IMAGES.contains(&crate::ui::files::extension(p).as_str())
    })?;
    Some(read_image(&path))
}

/// An image file's bytes and name. Err: it couldn't be read, or isn't an image this can use.
pub fn read_image(path: &std::path::Path) -> Result<(Vec<u8>, String), String> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    match std::fs::read(path) {
        Ok(bytes) if image::guess_format(&bytes).is_ok() => Ok((bytes, name)),
        Ok(_) => Err(format!("“{name}” isn't an image this can read (JPEG, PNG, WebP or GIF).")),
        Err(err) => Err(format!("Couldn't read “{name}”: {err}")),
    }
}

/// An image for a new cover: `chosen` in the file dialog, or (`taking`) pasted this frame
/// (Ctrl+V: an image in the clipboard, or a copied image file) or dropped on the window.
/// Its bytes and a file name. Err: an image file that couldn't be read or isn't one this
/// can use. Files that aren't images (by their extension) are left for others (notes).
pub fn take_image(ctx: &egui::Context, taking: bool, chosen: Vec<std::path::PathBuf>) -> Option<Result<(Vec<u8>, String), String>> {
    let pasted = taking
        && ctx.input(|i| {
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
    let mut files = chosen;
    if taking {
        files.extend(crate::ui::files::given(ctx));
    }
    for path in files.into_iter().filter(|p| crate::ui::files::IMAGES.contains(&crate::ui::files::extension(p).as_str())) {
        found = Some(read_image(&path));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn moving_a_cover_keeps_the_others_in_order() {
        let covers: Vec<Cover> = (1..=4)
            .map(|n| serde_json::from_value(json!({ "id": Uuid::from_u128(n), "blob_sha256": "", "source": "" })).unwrap())
            .collect();
        let id = Uuid::from_u128;
        assert_eq!(moved(&covers, 2, 0), [id(3), id(1), id(2), id(4)], "made main");
        assert_eq!(moved(&covers, 1, 2), [id(1), id(3), id(2), id(4)], "later");
    }

    #[test]
    fn what_a_draft_would_do_is_counted() {
        let covers: Vec<Cover> = (1..=3)
            .map(|n| serde_json::from_value(json!({ "id": Uuid::from_u128(n), "blob_sha256": "", "source": "" })).unwrap())
            .collect();
        let items = |covers: &[Cover]| -> Vec<Item> { covers.iter().map(|c| Item { what: What::Existing(c.clone()), trash: false, on_tracks: false }).collect() };
        assert_eq!(changes(&items(&covers), &covers), 0, "as the album has them");
        let mut moved = items(&covers);
        let item = moved.remove(2);
        moved.insert(0, item);
        assert_eq!(changes(&moved, &covers), 1, "another order");
        let mut more = items(&covers);
        more[1].trash = true;
        more[0].on_tracks = true;
        more.push(Item { what: What::Copy(covers[0].clone()), trash: false, on_tracks: false });
        assert_eq!(changes(&more, &covers), 3, "one trashed, one on the tracks, one added: the order is the album's still");
        let mut trashed_main = items(&covers);
        trashed_main[0].trash = true;
        assert_eq!(changes(&trashed_main, &covers), 1, "taking one out isn't a new order");
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

            let path = |id: &str| format!("/api/tracks/{id}/covers");
            let before = detail(track.clone()).await.covers.len();
            api.upload::<Value>(&path(&track), png, "test.png").await.unwrap();
            let covers = detail(track.clone()).await.covers;
            assert_eq!(covers.len(), before + 1);
            let new = covers.last().unwrap().clone();

            // Made the main one, as "Main" sends it.
            let order = moved(&covers, covers.len() - 1, 0);
            api.send(reqwest::Method::PUT, &format!("{}/order", path(&track)), Some(json!({ "cover_ids": order }))).await.unwrap();
            assert_eq!(detail(track.clone()).await.covers[0].id, new.id);

            // Copied to the other track: the same image.
            api.send(reqwest::Method::POST, &format!("{}/copy", path(&other)), Some(json!({ "cover_id": new.id }))).await.unwrap();
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
