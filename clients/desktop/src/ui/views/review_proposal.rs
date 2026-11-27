//! A proposal on the Review page: changes a tool suggests for the track (a script, a
//! language model), with what it found them in. Shown under the track's flags: each field
//! it would change, as it is and as proposed; a whole new version; sources it would add;
//! the evidence. Adopt makes the edit (`POST /api/review/{id}/adopt`), as you; if the
//! version changed since it was proposed, the server refuses it and the form below is
//! where to merge it by hand. Reject settles it with a reason, which the tool reads back
//! so it doesn't propose the same again.

use eframe::egui::{self, RichText};
use uuid::Uuid;

use crate::api::ApiError;
use crate::details::TrackDetail;
use crate::review::Flag;
use crate::ui::task::Task;
use crate::ui::theme::{font, medium, semibold, PALETTE as P};
use crate::ui::{Action, Shell};

#[derive(Default)]
pub struct Proposals {
    /// The proposal whose reason for rejecting it is being typed, and the reason.
    rejecting: Option<(Uuid, String)>,
    /// An adopt sent: (the flag, the track, the server's answer).
    adopting: Task<(Uuid, Uuid, Result<serde_json::Value, ApiError>)>,
    /// The last adopt's outcome, per flag (an error stays shown under it).
    outcome: Option<(Uuid, String)>,
}

impl Proposals {
    /// Shows `flag` (a proposal of `detail`'s track, if loaded). Returns a line for the page's
    /// notice when one was adopted or rejected.
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, flag: &Flag, detail: Option<&TrackDetail>) -> Option<String> {
        let mut notice = None;
        if let Some((id, track, answer)) = self.adopting.poll() {
            match answer {
                Ok(_) => {
                    shell.flags.take_out(&[id]);
                    shell.track_details.refresh(track);
                    shell.act(Action::RefreshLibrary);
                    notice = Some("Adopted.".to_string());
                }
                Err(err) if err.status == Some(409) => {
                    self.outcome = Some((id, "The track changed since this was proposed: change it by hand in the form below, then reject this.".into()));
                }
                Err(err) => self.outcome = Some((id, format!("Not adopted: {}", err.message))),
            }
        }

        let related = &flag.related;
        let agent = related.get("agent").and_then(|a| a.as_str()).unwrap_or("?");
        ui.label(RichText::new(format!("Proposed by {agent}")).font(font(13.0, semibold())).color(P.text));

        // Each field, as the version has it now and as proposed.
        let variant = related.get("variant_id").and_then(|v| v.as_str()).and_then(|v| v.parse::<Uuid>().ok());
        let base = related.get("base_revision_id").and_then(|v| v.as_str()).and_then(|v| v.parse::<Uuid>().ok());
        let current = detail.and_then(|d| d.variants.iter().find(|v| Some(v.id) == variant));
        if let Some(patch) = related.get("patch").and_then(|p| p.as_object()) {
            if current.is_some_and(|v| Some(v.revision_id) != base) {
                ui.label(RichText::new("This version changed since it was proposed.").color(P.danger).small());
            }
            egui::Grid::new(("proposal-fields", flag.id)).num_columns(3).spacing([12.0, 2.0]).show(ui, |ui| {
                for (field, proposed) in patch {
                    ui.label(RichText::new(field_name(field)).color(P.muted));
                    let now = current.and_then(|v| v.data.get(field)).cloned().unwrap_or(serde_json::Value::Null);
                    ui.label(RichText::new(shown(shell, &now)).color(P.faint).strikethrough());
                    ui.label(RichText::new(shown(shell, proposed)).color(P.text));
                    ui.end_row();
                }
            });
        }
        if let Some(version) = related.get("new_variant") {
            let label = version.get("label").and_then(|l| l.as_str()).unwrap_or("?");
            ui.label(RichText::new(format!("A new version, “{label}”:")).color(P.muted));
            if let Some(data) = version.get("data").and_then(|d| d.as_object()) {
                egui::Grid::new(("proposal-version", flag.id)).num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
                    for (field, value) in data {
                        ui.label(RichText::new(field_name(field)).color(P.muted));
                        ui.label(RichText::new(shown(shell, value)).color(P.text));
                        ui.end_row();
                    }
                });
            }
        }
        for source in related.get("sources").and_then(|s| s.as_array()).into_iter().flatten() {
            let platform = source.get("platform").and_then(|p| p.as_str()).unwrap_or("?");
            let data = source.get("data").map(|d| d.to_string()).unwrap_or_default();
            ui.label(RichText::new(format!("Source: {platform} {data}")).color(P.muted).small());
        }
        let evidence = related.get("evidence").and_then(|e| e.as_array()).cloned().unwrap_or_default();
        if !evidence.is_empty() {
            ui.label(RichText::new("Found in").font(font(12.0, medium())).color(P.muted));
            for e in &evidence {
                let label = e.get("label").and_then(|l| l.as_str()).unwrap_or("?");
                ui.horizontal_wrapped(|ui| {
                    match e.get("url").and_then(|u| u.as_str()) {
                        Some(url) => {
                            ui.hyperlink_to(label, url).on_hover_text(url);
                        }
                        None => {
                            ui.label(label);
                        }
                    }
                    if let Some(quote) = e.get("quote").and_then(|q| q.as_str()) {
                        ui.label(RichText::new(format!("“{quote}”")).color(P.faint).italics());
                    }
                });
            }
        }

        // Adopt, or reject with a reason.
        let new_version = related.get("new_variant").is_some();
        let busy = self.adopting.is_running();
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new("Adopt")).clicked() {
                self.adopt(shell, flag, false);
            }
            if new_version && ui.add_enabled(!busy, egui::Button::new("Adopt and show it")).on_hover_text("The new version becomes the one shown").clicked() {
                self.adopt(shell, flag, true);
            }
            if self.rejecting.as_ref().is_none_or(|(id, _)| *id != flag.id) && ui.button("Reject…").clicked() {
                self.rejecting = Some((flag.id, String::new()));
            }
        });
        if let Some((id, reason)) = &mut self.rejecting
            && *id == flag.id
        {
            let mut done = false;
            ui.horizontal(|ui| {
                let field = ui.add(egui::TextEdit::singleline(reason).hint_text("why (the tool reads it)").desired_width(260.0));
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button("Reject").clicked() || enter) && !reason.trim().is_empty() {
                    let resolution = format!("rejected: {}", reason.trim());
                    shell.flags.resolve(&shell.api, shell.runtime.handle(), &[flag.id], &resolution);
                    shell.act(Action::RefreshLibrary);
                    notice = Some("Rejected.".to_string());
                    done = true;
                }
                if ui.button("Cancel").clicked() {
                    done = true;
                }
            });
            if done {
                self.rejecting = None;
            }
        }
        if let Some((id, text)) = &self.outcome
            && *id == flag.id
        {
            ui.label(RichText::new(text).color(P.danger).small());
        }
        notice
    }

    fn adopt(&mut self, shell: &Shell, flag: &Flag, make_shown: bool) {
        let (id, track) = (flag.id, flag.track_id);
        self.outcome = None;
        let path = format!("/api/review/{id}/adopt");
        self.adopting.send_with(shell, move |api| async move {
            let body = serde_json::json!({ "make_shown": make_shown });
            (id, track, api.send_json::<serde_json::Value>(reqwest::Method::POST, &path, Some(body)).await)
        });
    }
}

fn field_name(field: &str) -> &str {
    match field {
        "title" => "Title",
        "release_date" => "Release date",
        "genres" => "Genres",
        "comment" => "Comment",
        "credits" => "Credits",
        other => other,
    }
}

/// A metadata value for people: credits by the artists' names.
fn shown(shell: &Shell, value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "(none)".into(),
        serde_json::Value::String(s) if s.is_empty() => "(empty)".into(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => {
            let parts: Vec<String> = items
                .iter()
                .map(|item| match item.get("artist_id").and_then(|a| a.as_str()).and_then(|a| a.parse::<Uuid>().ok()) {
                    Some(artist) => {
                        let name = shell.catalog.value.as_ref().and_then(|c| c.artist_label(artist)).unwrap_or_else(|| artist.to_string());
                        let role = item.get("role").and_then(|r| r.as_str()).unwrap_or("");
                        if role.is_empty() || role == "primary" { name } else { format!("{name} ({role})") }
                    }
                    None => shown(shell, item),
                })
                .collect();
            if parts.is_empty() { "(none)".into() } else { parts.join(", ") }
        }
        other => other.to_string(),
    }
}
