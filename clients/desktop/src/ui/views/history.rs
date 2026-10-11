//! A track's, album's or artist's history (its page, for users who may see it): every
//! revision, newest first,
//! with who, when, how (an edit, an import, a merge…) and why, and what it changed from
//! the one before it of the same kind (a version's metadata, or the track's facts), in
//! words: "title: “A” → “B”", names instead of ids. Restore (with `restore_revision`)
//! writes a new revision with that one's data; it asks first, and the ones in between
//! stay in the history.

use std::collections::HashMap;

use eframe::egui::{self, RichText};
use serde_json::Value;
use uuid::Uuid;

use crate::catalog::Catalog;
pub use acetate_core::editor::diff::changes_named;
use crate::details::{Revision, TrackDetail};
use crate::ui::task::Task;
use crate::ui::theme::{font, semibold, PALETTE as P};
use crate::ui::{Action, Shell};

/// Whose history: where a revision of it is restored, and what's asked for again after.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Of {
    Track(Uuid),
    Album(Uuid),
    Artist(Uuid),
}

impl Of {
    fn restore_path(self, revision: Uuid) -> String {
        match self {
            Of::Track(_) => format!("/api/revisions/{revision}/restore"),
            Of::Album(_) => format!("/api/album-revisions/{revision}/restore"),
            Of::Artist(_) => format!("/api/artist-revisions/{revision}/restore"),
        }
    }

    fn refresh(self, shell: &mut Shell) {
        match self {
            Of::Track(id) => shell.track_details.refresh(id),
            Of::Album(id) => shell.album_details.refresh(id),
            Of::Artist(id) => shell.artist_details.refresh(id),
        }
    }
}

#[derive(Default)]
pub struct History {
    /// "Restore…" was clicked on this revision: asking to be sure.
    asked: Option<Uuid>,
    restoring: Option<Uuid>,
    answer: Task<Result<(), String>>,
    result: Option<Result<(), String>>,
}

impl History {
    /// A track's: its metadata (per version) and its facts.
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog, d: &TrackDetail) {
        let Some(revisions) = &d.revisions else { return };
        let variant_label = |id: Option<Uuid>| d.variants.iter().find(|v| Some(v.id) == id).map(|v| v.label.clone());
        let what = |r: &Revision| match (r.kind.as_str(), variant_label(r.variant_id)) {
            ("external", _) => "status and albums · ".to_string(),
            (_, Some(label)) if d.variants.len() > 1 => format!("metadata ({label}) · "),
            _ => "metadata · ".to_string(),
        };
        self.show_revisions(ui, shell, catalog, Of::Track(d.id), revisions, &d.alias_names, what);
    }

    /// Any revisions, newest first (`names`: aliases credited, by id; `what`: what a
    /// revision is of, said before how it was made).
    #[allow(clippy::too_many_arguments)]
    pub fn show_revisions(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog, of: Of, revisions: &[Revision], names: &HashMap<Uuid, String>, what: impl Fn(&Revision) -> String) {
        if let Some(result) = self.answer.poll() {
            self.restoring = None;
            if result.is_ok() {
                of.refresh(shell);
                shell.act(Action::RefreshLibrary);
            }
            self.result = Some(result);
        }
        let can_restore = shell.edit_mode && shell.account.can("restore_revision");
        if revisions.is_empty() {
            ui.label(RichText::new("No revisions you can see.").color(P.muted));
        }
        for (n, r) in revisions.iter().enumerate() {
            // The one before it: the next older of the same kind (and version).
            let before = revisions[n + 1..].iter().find(|o| o.kind == r.kind && o.variant_id == r.variant_id);
            // The newest of its kind is what's there now: nothing to restore.
            let newest = !revisions[..n].iter().any(|o| o.kind == r.kind && o.variant_id == r.variant_id);
            let who = r.author.clone().unwrap_or_else(|| "the system".into());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(r.created_at.get(..16).unwrap_or(&r.created_at).replace('T', " ")).font(font(12.5, semibold())));
                ui.label(RichText::new(format!("{}{} by {who}", what(r), r.source)).color(P.muted));
                if newest {
                    ui.label(RichText::new("· now").color(P.accent));
                } else if can_restore && self.asked != Some(r.id) && self.restoring != Some(r.id) && ui.small_button("Restore…").clicked() {
                    self.asked = Some(r.id);
                    self.result = None;
                }
            });
            if !r.reason.is_empty() {
                ui.label(RichText::new(format!("“{}”", r.reason)).color(P.muted).italics());
            }
            for line in changes_named(before.map(|b| &b.data), &r.data, catalog, names) {
                ui.label(RichText::new(format!("  {line}")).color(P.text));
            }
            if self.asked == Some(r.id) {
                ui.label(RichText::new("Restoring writes a new revision with this one's data; the ones after it stay in the history (and can be restored too).").color(P.text));
                ui.horizontal(|ui| {
                    if ui.button("Restore it").clicked() {
                        self.asked = None;
                        self.restore(shell, of, r);
                    }
                    if ui.button("Cancel").clicked() {
                        self.asked = None;
                    }
                });
            }
            if self.restoring == Some(r.id) {
                ui.label(RichText::new("Restoring…").color(P.muted));
            }
            ui.add_space(6.0);
        }
        match &self.result {
            Some(Ok(())) => {
                ui.label(RichText::new("Restored.").color(P.accent));
            }
            Some(Err(err)) => {
                ui.label(RichText::new(format!("Couldn't restore it: {err}")).color(P.danger));
            }
            None => {}
        }
    }

    fn restore(&mut self, shell: &Shell, of: Of, r: &Revision) {
        self.restoring = Some(r.id);
        let path = of.restore_path(r.id);
        self.answer.send_with(shell, |api| async move {
            api.send_json::<Value>(reqwest::Method::POST, &path, Some(serde_json::json!({ "reason": "" })))
                .await
                .map(|_| ())
                .map_err(|err| err.message)
        });
    }
}

