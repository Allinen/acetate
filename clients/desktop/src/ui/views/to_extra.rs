//! "Turn into an extra…" on a track's page (edit mode): for something imported as a track
//! that isn't one (vocals, a stem, a clip). It becomes an extra with the track's file (the
//! same bytes), seen like the track, linked to the song it belongs to (chosen here) and
//! the track's artists. The track stays, with everything on it, but nobody sees it; the
//! extra's page makes it a track again. The server's preview first (what it takes, what it's
//! linked to, who sees it, which albums lose it), then the button. See the server's
//! `db::extras::track_to_extra`.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};
use serde::Deserialize;
use uuid::Uuid;

use crate::catalog::search::Part;
use crate::ui::edit::inline_search::{Choice, InlineSearch};
use crate::ui::theme::{font, semibold, PALETTE as P};
use crate::ui::{widgets, Action, Page, Shell};

#[derive(Debug, Clone, Deserialize)]
struct Linked {
    kind: String,
    title: String,
}

#[derive(Debug, Clone, Deserialize)]
struct AlbumLeft {
    title: String,
    alone: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct Access {
    restricted: bool,
    #[serde(default)]
    groups: Vec<Uuid>,
}

#[derive(Debug, Clone, Deserialize)]
struct Answer {
    extra_id: Uuid,
    source: String,
    file: String,
    links: Vec<Linked>,
    access: Access,
    albums: Vec<AlbumLeft>,
}

/// What the preview was asked for: (kind, belongs to).
type Asked = (String, Option<Uuid>);

pub struct ToExtra {
    track: Uuid,
    kind: String,
    name: String,
    description: String,
    source: String,
    /// The song it belongs to: (id, title).
    belongs_to: Option<(Uuid, String)>,
    search: InlineSearch,
    asked: Option<Asked>,
    preview: Arc<Mutex<Option<(Asked, Result<Answer, String>)>>>,
    loaded: Option<Result<Answer, String>>,
    sending: bool,
    done: Arc<Mutex<Option<Result<Answer, String>>>>,
    error: Option<String>,
}

impl ToExtra {
    pub fn open(track: Uuid, title: &str) -> ToExtra {
        ToExtra {
            track,
            kind: "vocals".into(),
            name: title.to_string(),
            description: String::new(),
            source: String::new(),
            belongs_to: None,
            search: InlineSearch::new(Part::Tracks),
            asked: None,
            preview: Arc::default(),
            loaded: None,
            sending: false,
            done: Arc::default(),
            error: None,
        }
    }

    fn body(&self, preview: bool) -> serde_json::Value {
        serde_json::json!({
            "kind": self.kind.trim(), "name": self.name.trim(), "description": self.description.trim(),
            "source": self.source.trim(), "belongs_to": self.belongs_to.as_ref().map(|b| b.0), "preview": preview,
        })
    }

    /// Asks for the preview when what it depends on changed.
    fn ask(&mut self, shell: &Shell) {
        let now: Asked = (self.kind.trim().to_string(), self.belongs_to.as_ref().map(|b| b.0));
        if self.asked.as_ref() == Some(&now) {
            return;
        }
        self.asked = Some(now.clone());
        self.loaded = None;
        let (api, ctx, into, body, track) = (shell.api.clone(), shell.ctx.clone(), self.preview.clone(), self.body(true), self.track);
        shell.runtime.spawn(async move {
            let result = api.send_json::<Answer>(reqwest::Method::POST, &format!("/api/tracks/{track}/to-extra"), Some(body)).await.map_err(|e| e.message);
            *into.lock().unwrap() = Some((now, result));
            ctx.request_repaint();
        });
    }

    /// The popup. True when it's over (closed, or done: the extra's page opens).
    pub fn show(&mut self, shell: &mut Shell) -> bool {
        self.ask(shell);
        let previewed = self.preview.lock().unwrap().take();
        if let Some((asked, result)) = previewed
            && Some(&asked) == self.asked.as_ref()
        {
            self.loaded = Some(result);
        }
        let finished = self.done.lock().unwrap().take();
        if let Some(result) = finished {
            self.sending = false;
            match result {
                Ok(done) => {
                    shell.track_details.refresh(self.track);
                    shell.act(Action::RefreshLibrary);
                    shell.act(Action::Go(Page::Extra(done.extra_id)));
                    return true;
                }
                Err(err) => self.error = Some(err),
            }
        }
        shell.modal_open = true;
        let ctx = shell.ctx.clone();
        let catalog = shell.catalog.value.clone();
        let kinds = crate::ui::edit::extras::kinds(shell);
        let mut go = false;
        let mut cancel = false;
        let closed = crate::ui::modal::show(&ctx, "to-extra", "Turn into an extra", egui::Vec2::new(820.0, 720.0), |ui| {
            ui.label("For something imported as a track that isn't one (vocals, a stem, a clip): it becomes an extra of the song it belongs to. The track isn't deleted: it stays with everything on it (its file, sources, plays), seen by nobody; the extra's page makes it a track again.");
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.label("Kind");
                for k in &kinds {
                    if widgets::chip(ui, k, self.kind == *k) {
                        self.kind = k.to_string();
                    }
                }
                ui.add(egui::TextEdit::singleline(&mut self.kind).desired_width(120.0));
            });
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(420.0));
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("Belongs to");
                match &self.belongs_to {
                    Some((_, title)) => {
                        ui.label(RichText::new(title.as_str()).font(font(14.0, semibold())));
                        if ui.small_button("×").clicked() {
                            self.belongs_to = None;
                        }
                    }
                    None => {
                        ui.label(RichText::new("(the song it's part of; optional)").color(P.faint));
                    }
                }
            });
            if self.belongs_to.is_none() {
                let hit = self.search.show(ui, shell, catalog.as_deref(), egui::Id::new("to-extra-song"), "the song: type its title", &[]);
                if let (Some((Choice::Hit(i), _)), Some(c)) = (hit, catalog.as_deref())
                    && let Some(t) = c.get_track(i)
                    && t.id() != self.track
                {
                    self.belongs_to = Some((t.id(), format!("{} · {}", t.title(), t.artist_display())));
                }
            }
            ui.add(egui::TextEdit::multiline(&mut self.description).hint_text("Description (optional)").desired_rows(2).desired_width(f32::INFINITY));
            ui.add(egui::TextEdit::singleline(&mut self.source).hint_text("Source (optional; empty: the track's own source is written in)").desired_width(f32::INFINITY));
            ui.add_space(8.0);

            ui.label(RichText::new("What happens").font(font(14.0, semibold())));
            match &self.loaded {
                None => {
                    ui.label(RichText::new("Asking the server…").color(P.muted));
                }
                Some(Err(err)) => {
                    ui.label(RichText::new(err.as_str()).color(P.danger));
                }
                Some(Ok(a)) => {
                    ui.label(format!("• It takes the track's file, “{}” (the same bytes: nothing is copied or deleted).", a.file));
                    let links: Vec<String> = a.links.iter().map(|l| format!("{} ({})", l.title, l.kind)).collect();
                    if !links.is_empty() {
                        ui.label(format!("• Linked to {}.", links.join(", ")));
                    }
                    let seen = if a.access.restricted { format!("restricted, like the track ({} groups)", a.access.groups.len()) } else { "seen by everyone, like the track".to_string() };
                    ui.label(format!("• {}.", capitalized(&seen)));
                    if !a.source.is_empty() && self.source.trim().is_empty() {
                        ui.label(format!("• Source: {}.", a.source));
                    }
                    ui.label("• The track leaves every list, search and playlist (and your stars and plays stay with it).");
                    for album in &a.albums {
                        if album.alone {
                            ui.label(RichText::new(format!("• “{}” had no other track: it's hidden too (back with it).", album.title)).color(P.accent));
                        } else {
                            ui.label(format!("• “{}” shows one track fewer.", album.title));
                        }
                    }
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let ready = matches!(self.loaded, Some(Ok(_))) && !self.sending && !self.name.trim().is_empty();
                go = ui.add_enabled(ready, egui::Button::new("Turn it into an extra")).clicked();
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
            let (api, ctx, into, body, track) = (shell.api.clone(), shell.ctx.clone(), self.done.clone(), self.body(false), self.track);
            shell.runtime.spawn(async move {
                let result = api.send_json::<Answer>(reqwest::Method::POST, &format!("/api/tracks/{track}/to-extra"), Some(body)).await.map_err(|e| e.message);
                *into.lock().unwrap() = Some(result);
                ctx.request_repaint();
            });
        }
        (closed || cancel) && !self.sending
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}
