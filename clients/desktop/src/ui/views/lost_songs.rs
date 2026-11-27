//! Lost songs (extras of kind "lost_song": songs whose audio the library doesn't have): the
//! rows an artist's page and the Review page list them in, and the little form that makes
//! one on an artist's page (edit mode). A lost song's own page is the extra's page.

use eframe::egui::{self, RichText, Sense};
use uuid::Uuid;

use crate::details::Extra;
use crate::ui::task::Task;
use crate::ui::theme::{font, medium, semibold, PALETTE as P};
use crate::ui::{widgets, Action, Page, Shell};

/// A lost song's line under its title: its status (unless it's simply lost), who it was
/// credited to, when, how long, and how many files are left of it.
pub fn facts(e: &Extra) -> String {
    let mut facts = Vec::new();
    if let Some(lost) = &e.lost {
        match lost.status.as_str() {
            "lost" => {}
            "snippet" => facts.push("only a snippet".to_string()),
            other => facts.push(other.to_string()),
        }
        if !lost.artists_shown.is_empty() {
            facts.push(lost.artists_shown.clone());
        }
        if let Some(date) = &lost.release_date {
            facts.push(date.clone());
        }
        if lost.duration_ms.is_some() {
            facts.push(widgets::length(lost.duration_ms));
        }
    }
    let files = e.files.iter().filter(|f| !f.hidden).count();
    if files > 0 {
        facts.push(crate::ui::edit::extras::files_text(files));
    }
    if !e.sources.is_empty() {
        let platforms: Vec<&str> = e.sources.iter().map(|s| s.platform.as_str()).collect();
        facts.push(platforms.join(", "));
    }
    facts.join("  ·  ")
}

/// Lost songs, a row each (the title opens its page).
pub fn rows(ui: &mut egui::Ui, shell: &mut Shell, songs: &[&Extra]) {
    for e in songs {
        let response = ui
            .vertical(|ui| {
                let title = ui.add(egui::Label::new(RichText::new(&e.name).font(font(14.5, semibold())).color(P.text)).sense(Sense::click()));
                let line = facts(e);
                if !line.is_empty() {
                    ui.label(RichText::new(line).font(font(12.5, medium())).color(P.muted));
                }
                title
            })
            .inner;
        if response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Its page: images, sources, what is known").clicked() {
            shell.act(Action::Go(Page::Extra(e.id)));
        }
        ui.add_space(6.0);
    }
}

/// "New lost song…" on an artist's page: a title (and what else is known), made with the
/// artist, then its page (to add images, a snippet, where it was).
#[derive(Default)]
pub struct NewLostSong {
    open: bool,
    title: String,
    artists_shown: String,
    date: String,
    answer: Task<Result<Uuid, String>>,
    error: Option<String>,
}

impl NewLostSong {
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, artist: Uuid) {
        if let Some(answer) = self.answer.poll() {
            match answer {
                Ok(id) => {
                    *self = NewLostSong::default();
                    shell.artist_details.refresh(artist);
                    shell.act(Action::RefreshLibrary);
                    shell.act(Action::Go(Page::Extra(id)));
                    return;
                }
                Err(err) => self.error = Some(err),
            }
        }
        if !self.open {
            if ui.small_button("New lost song…").on_hover_text("A song of theirs whose audio isn't here (deleted, unreleased, only a snippet…)").clicked() {
                self.open = true;
            }
            return;
        }
        let busy = self.answer.is_running();
        ui.add(egui::TextEdit::singleline(&mut self.title).hint_text("Title").desired_width(360.0));
        ui.add(egui::TextEdit::singleline(&mut self.artists_shown).hint_text("Artists as it was credited (optional, e.g. A ft. B)").desired_width(360.0));
        ui.add(egui::TextEdit::singleline(&mut self.date).hint_text("Date (optional): 2019, 2019-05 or 2019-05-12").desired_width(360.0));
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy && !self.title.trim().is_empty(), egui::Button::new("Make it")).clicked() {
                self.error = None;
                let body = serde_json::json!({
                    "name": self.title.trim(),
                    "artist_ids": [artist],
                    "lost": {
                        "status": "lost",
                        "artists_shown": self.artists_shown.trim(),
                        "release_date": Some(self.date.trim()).filter(|d| !d.is_empty()),
                    },
                });
                self.answer.send_with(shell, |api| async move {
                    api.send_json::<serde_json::Value>(reqwest::Method::POST, "/api/lost-songs", Some(body))
                        .await
                        .map_err(|e| e.message)
                        .and_then(|v| v["id"].as_str().and_then(|id| id.parse().ok()).ok_or_else(|| "no id in the answer".to_string()))
                });
            }
            if ui.button("Cancel").clicked() {
                *self = NewLostSong::default();
            }
        });
        if let Some(err) = &self.error {
            ui.label(RichText::new(err.as_str()).color(P.danger));
        }
    }
}
