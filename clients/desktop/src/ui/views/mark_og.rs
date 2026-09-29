//! "Mark as original files…" for the marked tracks (edit mode, `edit_files`): each track's
//! file becomes an original file (an og file), named as it was uploaded. For original files
//! whose tags didn't say so (ingest only knows from a tag). The server's preview first
//! (every track and the name it gets, nothing kept), a warning, then the button. See the
//! server's `db::og_files`.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText, Vec2};
use serde::Deserialize;
use uuid::Uuid;

use crate::ui::theme::{font, semibold, PALETTE as P};
use crate::ui::{thousands, widgets, Action, Shell};

#[derive(Debug, Clone, Deserialize)]
struct MarkedTrack {
    track_id: Uuid,
    title: String,
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Answer {
    tracks: Vec<MarkedTrack>,
    already: usize,
    not_found: usize,
}

pub struct MarkOg {
    tracks: Vec<Uuid>,
    reason: String,
    preview: Arc<Mutex<Option<Result<Answer, String>>>>,
    loaded: Option<Result<Answer, String>>,
    sending: bool,
    done: Arc<Mutex<Option<Result<Answer, String>>>>,
    outcome: Option<Result<String, String>>,
}

impl MarkOg {
    /// Asks the server what it would do.
    pub fn open(shell: &Shell, tracks: Vec<Uuid>) -> MarkOg {
        let popup = MarkOg { tracks, reason: String::new(), preview: Arc::default(), loaded: None, sending: false, done: Arc::default(), outcome: None };
        popup.send(shell, true);
        popup
    }

    /// The popup. True when it's closed.
    pub fn show(&mut self, shell: &mut Shell) -> bool {
        let previewed = self.preview.lock().unwrap().take();
        if let Some(result) = previewed {
            self.loaded = Some(result);
        }
        let finished = self.done.lock().unwrap().take();
        if let Some(result) = finished {
            self.sending = false;
            shell.act(Action::RefreshLibrary);
            self.outcome = Some(result.map(|a| format!("Done: {} tracks are original files now.", thousands(a.tracks.len()))));
            // Asked again: what's left to mark is nothing now.
            self.loaded = None;
            self.send(shell, true);
        }
        shell.modal_open = true;
        let ctx = shell.ctx.clone();
        let title = format!("Mark as original files: {} tracks", thousands(self.tracks.len()));
        let mut go = false;
        let mut cancel = false;
        let mut open = None;
        let closed = crate::ui::modal::show(&ctx, "mark-og", &title, Vec2::new(860.0, 720.0), |ui| {
            ui.label(RichText::new("Only do this if you know what you're doing!").font(font(15.0, semibold())).color(P.danger));
            ui.label("An original file (an og file) is the file as the artist made and named it, not a rip or a download. This says each marked track's file is one, named as it was uploaded (the name shown below). Only when you know where the files came from: nothing in a file can tell.");
            ui.label(RichText::new("Each change is kept with the file (who, when, why), and can be put back from the track's page.").color(P.muted));
            ui.add_space(8.0);
            let answer = match &self.loaded {
                None => {
                    ui.label(RichText::new("Asking the server what would change…").color(P.muted));
                    None
                }
                Some(Err(err)) => {
                    ui.label(RichText::new(err.as_str()).color(P.danger));
                    None
                }
                Some(Ok(answer)) => Some(answer.clone()),
            };
            if let Some(a) = &answer {
                let mut left = Vec::new();
                if a.already > 0 {
                    left.push(format!("{} are original files already", thousands(a.already)));
                }
                if a.not_found > 0 {
                    left.push(format!("{} aren't there anymore", thousands(a.not_found)));
                }
                ui.label(format!("{} tracks become original files.", thousands(a.tracks.len())));
                if !left.is_empty() {
                    ui.label(RichText::new(format!("Left as they are: {}.", left.join(", "))).color(P.muted));
                }
            }
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text("Why (optional: kept with every change)").desired_width(360.0));
                let count = answer.as_ref().map(|a| a.tracks.len()).unwrap_or(0);
                go = ui.add_enabled(!self.sending && count > 0, egui::Button::new(format!("Mark {} as original files", thousands(count)))).clicked();
                cancel = ui.button("Close").clicked();
                if self.sending {
                    ui.label(RichText::new("Working…").color(P.muted));
                }
            });
            match &self.outcome {
                Some(Ok(text)) => {
                    ui.label(RichText::new(text.as_str()).color(P.accent));
                }
                Some(Err(err)) => {
                    ui.label(RichText::new(err.as_str()).color(P.danger));
                }
                None => {}
            }
            if let Some(a) = answer
                && !a.tracks.is_empty()
            {
                ui.add_space(8.0);
                ui.label(RichText::new("The tracks, and the name each file gets").font(font(13.0, semibold())));
                egui::ScrollArea::vertical().id_salt("mark-og-tracks").auto_shrink([false, false]).show_rows(ui, 20.0, a.tracks.len(), |ui, range| {
                    for t in &a.tracks[range] {
                        ui.horizontal(|ui| {
                            if widgets::link(ui, &t.title, font(13.0, semibold()), P.text) {
                                open = Some(crate::ui::Page::Track(t.track_id));
                            }
                            ui.label(RichText::new(format!("→  {}", t.name)).color(P.accent));
                        });
                    }
                });
            }
        });
        if go {
            self.sending = true;
            self.outcome = None;
            self.send(shell, false);
        }
        if let Some(page) = open {
            shell.act(Action::Go(page));
            return true;
        }
        (closed || cancel) && !self.sending
    }

    fn send(&self, shell: &Shell, preview: bool) {
        let body = serde_json::json!({ "track_ids": self.tracks, "reason": self.reason.trim(), "preview": preview });
        let (api, ctx, answer) = (shell.api.clone(), shell.ctx.clone(), if preview { self.preview.clone() } else { self.done.clone() });
        shell.runtime.spawn(async move {
            let result = api.send_json::<Answer>(reqwest::Method::POST, "/api/tracks/mark-original", Some(body)).await.map_err(|e| e.message);
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }
}
