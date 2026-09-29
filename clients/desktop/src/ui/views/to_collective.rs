//! "Turn into a collective…" on an artist's page (edit mode): for an artist that is really
//! a collective (an account several artists post under, made an artist by an import from
//! before there were groupings). A popup shows the server's preview first, changing
//! nothing: the sources (accounts) that move to the collective, how many tracks and albums
//! go in it and lose the artist's credit, and which are left with no one credited (the
//! server flags those for Review). Then the button. See the server's
//! `db::groupings::turn_into_collective`.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};
use serde::Deserialize;
use uuid::Uuid;

use crate::ui::theme::{font, semibold, PALETTE as P};
use crate::ui::{thousands, widgets, Action, Page, Shell};

#[derive(Debug, Clone, Deserialize)]
struct Named {
    id: Uuid,
    title: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Preview {
    name: String,
    sources: Vec<crate::details::Source>,
    tracks: i64,
    albums: i64,
    tracks_left_without_primary: Vec<Named>,
    albums_left_without_artists: Vec<Named>,
}

pub struct ToCollective {
    artist: Uuid,
    preview: Arc<Mutex<Option<Result<Preview, String>>>>,
    loaded: Option<Result<Preview, String>>,
    reason: String,
    sending: bool,
    done: Arc<Mutex<Option<Result<Uuid, String>>>>,
    error: Option<String>,
}

impl ToCollective {
    /// Asks the server what it would do.
    pub fn open(shell: &Shell, artist: Uuid) -> ToCollective {
        let preview: Arc<Mutex<Option<Result<Preview, String>>>> = Arc::default();
        let (api, answer, ctx) = (shell.api.clone(), preview.clone(), shell.ctx.clone());
        shell.runtime.spawn(async move {
            let result = api.send_json::<Preview>(reqwest::Method::GET, &format!("/api/artists/{artist}/collective"), None).await.map_err(|e| e.message);
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
        ToCollective { artist, preview, loaded: None, reason: String::new(), sending: false, done: Arc::default(), error: None }
    }

    /// The popup. True when it's over (closed, or done: then its collective's page opens).
    pub fn show(&mut self, shell: &mut Shell) -> bool {
        if let Some(result) = self.preview.lock().unwrap().take() {
            self.loaded = Some(result);
        }
        if let Some(result) = self.done.lock().unwrap().take() {
            self.sending = false;
            match result {
                Ok(grouping) => {
                    shell.artist_details.refresh(self.artist);
                    shell.act(Action::RefreshLibrary);
                    shell.act(Action::Go(Page::Grouping(grouping)));
                    return true;
                }
                Err(err) => self.error = Some(err),
            }
        }
        shell.modal_open = true;
        let ctx = shell.ctx.clone();
        let mut go = false;
        let mut cancel = false;
        let closed = crate::ui::modal::show(&ctx, "to-collective", "Turn into a collective", egui::Vec2::new(760.0, 620.0), |ui| {
            let preview = match &self.loaded {
                None => {
                    ui.label(RichText::new("Asking the server what it would do…").color(P.muted));
                    return;
                }
                Some(Err(err)) => {
                    ui.label(RichText::new(err.as_str()).color(P.danger));
                    return;
                }
                Some(Ok(preview)) => preview.clone(),
            };
            egui::ScrollArea::vertical().max_height(440.0).show(ui, |ui| {
                ui.label(format!(
                    "“{}” becomes a collective: a grouping, not an artist. Credits say who made a song; a collective is where it was posted.",
                    preview.name
                ));
                ui.add_space(8.0);
                ui.label(RichText::new("What happens").font(font(14.0, semibold())));
                let accounts = if preview.sources.is_empty() {
                    "It has no sources (accounts): songs from its account won't go in by themselves until the collective gets one.".to_string()
                } else {
                    let list: Vec<String> = preview.sources.iter().map(|s| format!("{} {}", s.platform, s.external_id.as_deref().unwrap_or(""))).collect();
                    format!("Its sources move to the collective ({}): songs imported from those accounts go in it from now on, credited to no one as it.", list.join(", "))
                };
                ui.label(format!("• {accounts}"));
                ui.label(format!(
                    "• {} tracks and {} albums go in the collective, and its credit comes off them (a new revision each, restorable on each one's page).",
                    thousands(preview.tracks as usize),
                    thousands(preview.albums as usize)
                ));
                ui.label("• The artist stays (nothing's deleted), credited nowhere.");
                if !preview.tracks_left_without_primary.is_empty() {
                    ui.add_space(8.0);
                    ui.label(RichText::new(format!("Left with no primary artist ({}): name who made them, on Review", preview.tracks_left_without_primary.len())).font(font(14.0, semibold())).color(P.danger));
                    for t in preview.tracks_left_without_primary.iter().take(50) {
                        if widgets::link(ui, &t.title, font(13.0, semibold()), P.muted) {
                            shell.act(Action::Go(Page::Track(t.id)));
                        }
                    }
                    if preview.tracks_left_without_primary.len() > 50 {
                        ui.label(RichText::new(format!("and {} more", preview.tracks_left_without_primary.len() - 50)).color(P.faint));
                    }
                }
                if !preview.albums_left_without_artists.is_empty() {
                    ui.add_space(8.0);
                    ui.label(RichText::new(format!("Albums left with no artists ({})", preview.albums_left_without_artists.len())).font(font(14.0, semibold())));
                    let titles: Vec<&str> = preview.albums_left_without_artists.iter().take(30).map(|a| a.title.as_str()).collect();
                    ui.label(RichText::new(titles.join(" · ")).color(P.muted));
                }
            });
            ui.add_space(8.0);
            ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text("Why (optional: kept with every change)").desired_width(460.0));
            ui.horizontal(|ui| {
                go = ui.add_enabled(!self.sending, egui::Button::new("Turn it into a collective")).clicked();
                cancel = ui.button("Cancel").clicked();
                if self.sending {
                    ui.label(RichText::new("Working…").color(P.muted));
                }
            });
            if let Some(err) = &self.error {
                ui.label(RichText::new(err.as_str()).color(P.danger));
            }
        });
        if go {
            self.sending = true;
            self.error = None;
            #[derive(Deserialize)]
            struct Done {
                grouping_id: Uuid,
            }
            let body = serde_json::json!({ "reason": self.reason.trim() });
            let (api, answer, ctx, artist) = (shell.api.clone(), self.done.clone(), shell.ctx.clone(), self.artist);
            shell.runtime.spawn(async move {
                let result = api
                    .send_json::<Done>(reqwest::Method::POST, &format!("/api/artists/{artist}/collective"), Some(body))
                    .await
                    .map(|d| d.grouping_id)
                    .map_err(|e| e.message);
                *answer.lock().unwrap() = Some(result);
                ctx.request_repaint();
            });
        }
        (closed || cancel) && !self.sending
    }
}
