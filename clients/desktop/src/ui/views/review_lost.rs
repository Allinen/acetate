//! Lost songs on the Review page: a track that may be a song listed as lost (a
//! `lost_song_found` flag: the two share a platform id), shown with the lost song, to say
//! it's that song (`POST /api/review/{id}/found`: the track gets the lost song's sources and
//! the lost song is taken off) or not; and the lost songs a tool proposed, to accept or take
//! off on their pages.

use eframe::egui::{self, RichText, Sense};
use uuid::Uuid;

use crate::catalog::Catalog;
use crate::review::Flag;
use crate::ui::task::Task;
use crate::ui::theme::{font, medium, semibold, PALETTE as P};
use crate::ui::{Action, Page, Shell};

#[derive(Default)]
pub struct LostFound {
    /// An answer sent: (the flag, the track, what it says when done, the server's answer).
    sent: Task<(Uuid, Uuid, &'static str, Result<serde_json::Value, String>)>,
    /// The last one's error, per flag.
    error: Option<(Uuid, String)>,
}

impl LostFound {
    /// Shows `flag` (a `lost_song_found` flag). Returns a line for the page's notice when it
    /// was answered.
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog, flag: &Flag) -> Option<String> {
        let mut notice = None;
        if let Some((id, track, done, answer)) = self.sent.poll() {
            match answer {
                Ok(_) => {
                    shell.flags.take_out(&[id]);
                    shell.track_details.refresh(track);
                    shell.act(Action::RefreshLibrary);
                    notice = Some(done.to_string());
                }
                Err(err) => self.error = Some((id, err)),
            }
        }
        let extra = flag.related.get("extra_id").and_then(|v| v.as_str()).and_then(|v| v.parse::<Uuid>().ok());
        let platform = flag.related.get("platform").and_then(|v| v.as_str()).unwrap_or("?");
        let external = flag.related.get("external_id").and_then(|v| v.as_str()).unwrap_or("?");
        let known = extra.and_then(|id| catalog.extras.iter().find(|e| e.id == id));
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("May be the lost song").color(P.text));
            match (extra, known) {
                (Some(id), Some(e)) => {
                    let name = ui.add(egui::Label::new(RichText::new(format!("“{}”", e.name)).font(font(13.5, semibold())).color(P.accent)).sense(Sense::click()));
                    if name.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Its page").clicked() {
                        shell.act(Action::Go(Page::Extra(id)));
                    }
                }
                _ => {
                    ui.label(RichText::new("(one you can't see, or taken off since)").color(P.muted));
                }
            }
            ui.label(RichText::new(format!("(both have {platform} {external})")).font(font(12.5, medium())).color(P.muted));
        });
        if let Some(lost) = known.and_then(|e| e.lost.as_ref()) {
            let mut facts = vec![lost.status.clone()];
            facts.extend([lost.artists_shown.clone()].into_iter().filter(|a| !a.is_empty()));
            facts.extend(lost.release_date.clone());
            ui.label(RichText::new(facts.join("  ·  ")).color(P.faint).small());
        }
        let busy = self.sent.is_running();
        ui.horizontal(|ui| {
            let (id, track) = (flag.id, flag.track_id);
            if ui.add_enabled(!busy && known.is_some(), egui::Button::new("It's this song")).on_hover_text("The track gets the lost song's sources, and the lost song is taken off (kept)").clicked() {
                self.error = None;
                self.sent.send_with(shell, |api| async move {
                    let answer = api.send_json::<serde_json::Value>(reqwest::Method::POST, &format!("/api/review/{id}/found"), Some(serde_json::json!({}))).await.map_err(|e| e.message);
                    (id, track, "Found: the lost song is taken off.", answer)
                });
            }
            if ui.add_enabled(!busy, egui::Button::new("Not this song")).clicked() {
                self.error = None;
                self.sent.send_with(shell, |api| async move {
                    let body = serde_json::json!({ "resolution": "not this song" });
                    let answer = api.send_json::<serde_json::Value>(reqwest::Method::POST, &format!("/api/review/{id}/resolve"), Some(body)).await.map_err(|e| e.message);
                    (id, track, "Not that song: settled.", answer)
                });
            }
        });
        if let Some((id, err)) = &self.error
            && *id == flag.id
        {
            ui.label(RichText::new(err.as_str()).color(P.danger).small());
        }
        notice
    }
}

/// The lost songs a tool proposed (not taken off), folded under a heading; each opens its
/// page, where it's accepted or taken off. Nothing when there are none.
pub fn proposed(ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog) {
    let mut songs: Vec<&crate::catalog::model::Extra> =
        catalog.extras.iter().filter(|e| !e.hidden && e.lost.as_ref().is_some_and(|l| l.status == "proposed")).collect();
    if songs.is_empty() {
        return;
    }
    songs.sort_by_key(|e| e.name.to_lowercase());
    egui::CollapsingHeader::new(RichText::new(format!("Proposed lost songs ({})", songs.len())).font(font(14.0, semibold()))).id_salt("review-proposed-lost").show(ui, |ui| {
        ui.label(RichText::new("Songs a tool found that aren't here: open one to accept it or take it off.").color(P.faint).small());
        for e in songs {
            let lost = e.lost.as_ref();
            let mut line = vec![e.name.clone()];
            line.extend(lost.map(|l| l.artists_shown.clone()).filter(|a| !a.is_empty()));
            line.extend(lost.and_then(|l| l.release_date.clone()));
            let row = ui.add(egui::Label::new(RichText::new(line.join("  ·  ")).color(P.text)).sense(Sense::click()));
            if row.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                shell.act(Action::Go(Page::Extra(e.id)));
            }
        }
    });
    ui.add_space(4.0);
}
