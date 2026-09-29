//! On a track's page (File): whether its file is an original file (an og file), corrected
//! by hand (edit mode, `edit_files`: a very specific fix, for someone who knows where the
//! files came from), and the changes of it, each one put back with a click.
//! `PUT /api/files/{id}/original-filename`; see the server's `db::og_files`.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};
use uuid::Uuid;

use crate::details::TrackDetail;
use crate::ui::theme::{font, semibold, PALETTE as P};
use crate::ui::{Action, Shell};

/// The correction being made (kept in egui's memory, per file).
#[derive(Default)]
struct Editing {
    open: bool,
    name: String,
    reason: String,
    sending: bool,
    answer: Option<Result<(), String>>,
}

type State = Arc<Mutex<Editing>>;

/// The history of it (anyone who may see history), and the correction (edit mode, with
/// `edit_files`).
pub fn show(ui: &mut egui::Ui, shell: &mut Shell, d: &TrackDetail) {
    let can = shell.edit_mode && shell.account.can("edit_files");
    let changes = d.original_filename_changes.as_deref().unwrap_or_default();
    let state: State = ui.ctx().data_mut(|m| m.get_temp_mut_or_default::<State>(egui::Id::new(("og-file", d.file.id))).clone());
    let mut put_back: Option<Option<String>> = None;
    if !changes.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new("Original file: changes").font(font(14.0, semibold())));
        let named = |name: &Option<String>| name.as_deref().map(|n| format!("original file “{n}”")).unwrap_or_else(|| "not an original file".into());
        for (n, c) in changes.iter().enumerate() {
            ui.horizontal_wrapped(|ui| {
                let by = c.changed_by.as_deref().map(|b| format!(" by {b}")).unwrap_or_default();
                let why = if c.reason.is_empty() { String::new() } else { format!(" · “{}”", c.reason) };
                ui.label(format!("{}{by}: {} → {}{why}", c.changed_at.get(..10).unwrap_or(&c.changed_at), named(&c.old_name), named(&c.new_name)));
                // The newest can be undone (what it was before).
                if can && n == 0 && ui.small_button("Put back").on_hover_text(format!("Back to: {}", named(&c.old_name))).clicked() {
                    put_back = Some(c.old_name.clone());
                }
            });
        }
    }
    if !can {
        return;
    }
    let mut guard = state.lock().unwrap();
    let e = &mut *guard;
    ui.add_space(8.0);
    if !e.open {
        let text = if d.file.original_filename.is_some() { "Correct the original file…" } else { "It's an original file…" };
        if ui.button(text).on_hover_text("Whether this file is the one the artist made and named (an og file). Only if you know where it came from").clicked() {
            e.open = true;
            e.name = d.file.original_filename.clone().unwrap_or_else(|| d.file.uploaded_filename.clone());
            e.answer = None;
        }
    } else {
        ui.label(RichText::new("Only do this if you know what you're doing!").font(font(13.5, semibold())).color(P.danger));
        ui.label(RichText::new("An original file is the file as the artist made and named it, not a rip or a download. Nothing in a file can tell: only where it came from.").color(P.muted));
        ui.horizontal(|ui| {
            ui.label("Its name");
            ui.add(egui::TextEdit::singleline(&mut e.name).desired_width(360.0));
        });
        ui.add(egui::TextEdit::singleline(&mut e.reason).hint_text("Why (optional: kept with the change)").desired_width(420.0));
        ui.horizontal(|ui| {
            if ui.add_enabled(!e.sending && !e.name.trim().is_empty(), egui::Button::new("It's an original file, named so")).clicked() {
                put_back = Some(Some(e.name.trim().to_string()));
            }
            if d.file.original_filename.is_some() && ui.add_enabled(!e.sending, egui::Button::new("Not an original file")).clicked() {
                put_back = Some(None);
            }
            if ui.button("Cancel").clicked() {
                e.open = false;
            }
            if e.sending {
                ui.label(RichText::new("Saving…").color(P.muted));
            }
        });
    }
    match e.answer.take() {
        Some(Ok(())) => {
            e.open = false;
            e.reason.clear();
            shell.track_details.refresh(d.id);
            shell.act(Action::RefreshLibrary);
        }
        Some(Err(err)) => {
            ui.label(RichText::new(err.as_str()).color(P.danger));
            e.answer = Some(Err(err));
        }
        None => {}
    }
    let to_send = match put_back {
        Some(name) if !e.sending => {
            e.sending = true;
            e.answer = None;
            Some((name, e.reason.trim().to_string()))
        }
        _ => None,
    };
    // (The answer takes the lock: let go of it first.)
    drop(guard);
    if let Some((name, reason)) = to_send {
        send(shell, &state, d.file.id, name, reason);
    }
}

fn send(shell: &Shell, state: &State, file: Uuid, name: Option<String>, reason: String) {
    let (api, ctx, state) = (shell.api.clone(), shell.ctx.clone(), state.clone());
    shell.runtime.spawn(async move {
        let body = serde_json::json!({ "original_filename": name, "reason": reason });
        let result = api.send_json::<serde_json::Value>(reqwest::Method::PUT, &format!("/api/files/{file}/original-filename"), Some(body)).await;
        let mut e = state.lock().unwrap();
        e.sending = false;
        e.answer = Some(result.map(|_| ()).map_err(|err| err.message));
        ctx.request_repaint();
    });
}
