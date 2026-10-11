//! "Turn into a collective…" on an artist's page (edit mode): for an artist that is really
//! a collective (an account several artists post under, made an artist by an import from
//! before there were groupings). Into a new collective (its name), or merged into one there
//! is already (the same collective, whose account on another platform made another
//! artist of it). A popup shows the server's preview first, changing
//! nothing: the sources (accounts) that move to the collective, how many tracks and albums
//! go in it and lose the artist's credit, which are left with no one credited (the
//! server flags those for Review), and the extras that move to it. Then the button. See the server's
//! `db::groupings::turn_into_collective`.


use eframe::egui::{self, RichText};
use serde::Deserialize;
use uuid::Uuid;

use crate::ui::task::Task;
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
    into: Option<Named>,
    sources: Vec<crate::details::Source>,
    tracks: i64,
    albums: i64,
    tracks_left_without_primary: Vec<Named>,
    albums_left_without_artists: Vec<Named>,
    #[serde(default)]
    extras: Vec<Named>,
}

pub struct ToCollective {
    artist: Uuid,
    /// An existing collective it goes into (None: a new one).
    into: Option<Uuid>,
    /// The preview's answer, with the target it's for (one for a target no longer chosen
    /// is dropped).
    preview: Task<(Option<Uuid>, Result<Preview, String>)>,
    loaded: Option<Result<Preview, String>>,
    reason: String,
    sending: bool,
    done: Task<Result<Uuid, String>>,
    error: Option<String>,
}

impl ToCollective {
    /// Asks the server what it would do.
    pub fn open(shell: &Shell, artist: Uuid) -> ToCollective {
        let mut popup = ToCollective { artist, into: None, preview: Task::default(), loaded: None, reason: String::new(), sending: false, done: Task::default(), error: None };
        popup.ask(shell);
        popup
    }

    /// The preview, for a new collective or the one chosen.
    fn ask(&mut self, shell: &Shell) {
        self.loaded = None;
        let query = self.into.map(|g| format!("?into={g}")).unwrap_or_default();
        let (artist, into) = (self.artist, self.into);
        self.preview.start_with(shell, |api| async move {
            let result = api.send_json::<Preview>(reqwest::Method::GET, &format!("/api/artists/{artist}/collective{query}"), None).await.map_err(|e| e.message);
            (into, result)
        });
    }

    /// The popup. True when it's over (closed, or done: then its collective's page opens).
    pub fn show(&mut self, shell: &mut Shell) -> bool {
        if let Some((into, result)) = self.preview.poll()
            && into == self.into
        {
            self.loaded = Some(result);
        }
        if let Some(result) = self.done.poll() {
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
        let mut choose: Option<Option<Uuid>> = None;
        let catalog = shell.catalog.value.clone();
        let closed = crate::ui::modal::show(&ctx, "to-collective", "Turn into a collective", egui::Vec2::new(760.0, 660.0), |ui| {
            // Into a new one, or one there is (collectives first, then other groupings).
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Into").color(P.faint));
                let chosen = self.into.and_then(|g| catalog.as_ref()?.grouping(g).map(|g| format!("{} ({})", g.name, g.kind))).unwrap_or_else(|| "a new collective, with its name".into());
                egui::ComboBox::from_id_salt("to-collective-into").selected_text(chosen).width(360.0).show_ui(ui, |ui| {
                    if ui.selectable_label(self.into.is_none(), "a new collective, with its name").clicked() {
                        choose = Some(None);
                    }
                    if let Some(c) = &catalog {
                        let mut groupings: Vec<_> = c.groupings.iter().collect();
                        groupings.sort_by_key(|g| (g.kind != "collective", g.name.to_lowercase()));
                        for g in groupings {
                            if ui.selectable_label(self.into == Some(g.id), format!("{} ({})", g.name, g.kind)).clicked() {
                                choose = Some(Some(g.id));
                            }
                        }
                    }
                });
            });
            ui.add_space(6.0);
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
                match &preview.into {
                    Some(into) => ui.label(format!(
                        "“{}” is merged into the collective “{}”: the same collective, another account. Credits say who made a song; a collective is where it was posted.",
                        preview.name, into.title
                    )),
                    None => ui.label(format!(
                        "“{}” becomes a collective: a grouping, not an artist. Credits say who made a song; a collective is where it was posted.",
                        preview.name
                    )),
                };
                ui.add_space(8.0);
                ui.label(RichText::new("What happens").font(font(14.0, semibold())));
                let accounts = if preview.sources.is_empty() {
                    "It has no sources (accounts): songs from its account won't go in by themselves until the collective gets one.".to_string()
                } else {
                    let list: Vec<String> = preview.sources.iter().map(|s| format!("{} {}", s.platform, s.external_id.as_deref().unwrap_or(""))).collect();
                    format!("Its sources move to the collective ({}; one it has already joins that one): songs imported from those accounts go in it from now on, credited to no one as it.", list.join(", "))
                };
                ui.label(format!("• {accounts}"));
                ui.label(format!(
                    "• {} tracks and {} albums go in the collective, and its credit comes off them (a new revision each, restorable on each one's page).",
                    thousands(preview.tracks as usize),
                    thousands(preview.albums as usize)
                ));
                if !preview.extras.is_empty() {
                    let names: Vec<&str> = preview.extras.iter().take(30).map(|e| e.title.as_str()).collect();
                    let more = if preview.extras.len() > 30 { format!(" and {} more", preview.extras.len() - 30) } else { String::new() };
                    ui.label(format!("• Its extras ({}) belong to the collective instead: {}{more}.", thousands(preview.extras.len()), names.join(", ")));
                }
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
                let text = if self.into.is_some() { "Merge it into the collective" } else { "Turn it into a collective" };
                go = ui.add_enabled(!self.sending && matches!(self.loaded, Some(Ok(_))), egui::Button::new(text)).clicked();
                cancel = ui.button("Cancel").clicked();
                if self.sending {
                    ui.label(RichText::new("Working…").color(P.muted));
                }
            });
            if let Some(err) = &self.error {
                ui.label(RichText::new(err.as_str()).color(P.danger));
            }
        });
        if let Some(into) = choose
            && into != self.into
        {
            self.into = into;
            self.ask(shell);
        }
        if go {
            self.sending = true;
            self.error = None;
            #[derive(Deserialize)]
            struct Done {
                grouping_id: Uuid,
            }
            let body = serde_json::json!({ "reason": self.reason.trim(), "into": self.into });
            let artist = self.artist;
            self.done.send_with(shell, |api| async move {
                api.send_json::<Done>(reqwest::Method::POST, &format!("/api/artists/{artist}/collective"), Some(body))
                    .await
                    .map(|d| d.grouping_id)
                    .map_err(|e| e.message)
            });
        }
        (closed || cancel) && !self.sending
    }
}
