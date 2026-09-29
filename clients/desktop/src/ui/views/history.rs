//! A track's history (its page, for users who may see it): every revision, newest first,
//! with who, when, how (an edit, an import, a merge…) and why, and what it changed from
//! the one before it of the same kind (a version's metadata, or the track's facts), in
//! words: "title: “A” → “B”", names instead of ids. Restore (with `restore_revision`)
//! writes a new revision with that one's data; it asks first, and the ones in between
//! stay in the history.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};
use serde_json::Value;
use uuid::Uuid;

use crate::catalog::Catalog;
use crate::details::{Revision, TrackDetail};
use crate::ui::theme::{font, semibold, PALETTE as P};
use crate::ui::{Action, Shell};

#[derive(Default)]
pub struct History {
    /// "Restore…" was clicked on this revision: asking to be sure.
    asked: Option<Uuid>,
    restoring: Option<Uuid>,
    answer: Arc<Mutex<Option<Result<(), String>>>>,
    result: Option<Result<(), String>>,
}

impl History {
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog, d: &TrackDetail) {
        if let Some(result) = self.answer.lock().unwrap().take() {
            self.restoring = None;
            if result.is_ok() {
                shell.track_details.refresh(d.id);
                shell.act(Action::RefreshLibrary);
            }
            self.result = Some(result);
        }
        let Some(revisions) = &d.revisions else { return };
        let can_restore = shell.account.can("restore_revision");
        let variant_label = |id: Option<Uuid>| d.variants.iter().find(|v| Some(v.id) == id).map(|v| v.label.clone());
        for (n, r) in revisions.iter().enumerate() {
            // The one before it: the next older of the same kind (and version).
            let before = revisions[n + 1..].iter().find(|o| o.kind == r.kind && o.variant_id == r.variant_id);
            // The newest of its kind is what's there now: nothing to restore.
            let newest = !revisions[..n].iter().any(|o| o.kind == r.kind && o.variant_id == r.variant_id);
            let what = match (r.kind.as_str(), variant_label(r.variant_id)) {
                ("external", _) => "status and albums".to_string(),
                (_, Some(label)) if d.variants.len() > 1 => format!("metadata ({label})"),
                _ => "metadata".to_string(),
            };
            let who = r.author.clone().unwrap_or_else(|| "the system".into());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(r.created_at.get(..16).unwrap_or(&r.created_at).replace('T', " ")).font(font(12.5, semibold())));
                ui.label(RichText::new(format!("{what} · {} by {who}", r.source)).color(P.muted));
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
            for line in changes(before.map(|b| &b.data), &r.data, catalog) {
                ui.label(RichText::new(format!("  {line}")).color(P.text));
            }
            if self.asked == Some(r.id) {
                ui.label(RichText::new("Restoring writes a new revision with this one's data; the ones after it stay in the history (and can be restored too).").color(P.text));
                ui.horizontal(|ui| {
                    if ui.button("Restore it").clicked() {
                        self.asked = None;
                        self.restore(shell, r);
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

    fn restore(&mut self, shell: &Shell, r: &Revision) {
        self.restoring = Some(r.id);
        let (api, answer, ctx, id) = (shell.api.clone(), self.answer.clone(), shell.ctx.clone(), r.id);
        shell.runtime.spawn(async move {
            let result = api
                .send_json::<Value>(reqwest::Method::POST, &format!("/api/revisions/{id}/restore"), Some(serde_json::json!({ "reason": "" })))
                .await
                .map(|_| ())
                .map_err(|err| err.message);
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }
}

/// What `after` changed from `before` (None: it's the first), in words.
pub fn changes(before: Option<&Value>, after: &Value, catalog: &Catalog) -> Vec<String> {
    changes_named(before, after, catalog, &HashMap::new())
}

/// Like `changes`, with names for ids not in the library (yet): a bulk edit's preview of
/// artists and albums it would make.
pub fn changes_named(before: Option<&Value>, after: &Value, catalog: &Catalog, names: &HashMap<Uuid, String>) -> Vec<String> {
    let Some(before) = before else { return vec!["the first one".into()] };
    let fields: [(&str, &str); 8] = [
        ("title", "title"),
        ("release_date", "released"),
        ("genres", "genres"),
        ("comment", "comment"),
        ("credits", "artists"),
        ("status", "status"),
        ("albums", "albums"),
        ("relations", "related"),
    ];
    let mut lines = Vec::new();
    for (key, name) in fields {
        let (was, now) = (before.get(key), after.get(key));
        let empty = |v: Option<&Value>| v.is_none_or(|v| v.is_null() || v.as_array().is_some_and(|a| a.is_empty()) || v.as_str() == Some(""));
        if was == now || (empty(was) && empty(now)) {
            continue;
        }
        lines.push(format!("{name}: {} → {}", describe(key, was, catalog, names), describe(key, now, catalog, names)));
    }
    if lines.is_empty() {
        lines.push("nothing different".into());
    }
    lines
}

/// A field's value in words (lists joined; ids as names).
fn describe(key: &str, value: Option<&Value>, catalog: &Catalog, names: &HashMap<Uuid, String>) -> String {
    let Some(value) = value.filter(|v| !v.is_null()) else { return "none".into() };
    let id = |v: &Value, field: &str| v.get(field).and_then(|x| x.as_str()).and_then(|x| x.parse::<Uuid>().ok());
    let list: Vec<String> = match (key, value.as_array()) {
        ("credits", Some(items)) => items
            .iter()
            .map(|c| {
                let artist = id(c, "artist_id");
                let name = artist
                    .and_then(|a| catalog.artist(a).map(|a| a.name.clone()).or_else(|| names.get(&a).map(|n| format!("{n} (new)"))))
                    .unwrap_or_else(|| "(an artist)".into());
                match c.get("role").and_then(|r| r.as_str()) {
                    Some("primary") | None => name,
                    Some(role) => format!("{name} ({role})"),
                }
            })
            .collect(),
        ("albums", Some(items)) => items
            .iter()
            .map(|a| {
                let album = id(a, "album_id");
                let title = album
                    .and_then(|x| catalog.album(x).map(|x| format!("“{}”", x.title)).or_else(|| names.get(&x).map(|n| format!("“{n}” (new)"))))
                    .unwrap_or_else(|| "(an album)".into());
                match (a.get("disc_number").and_then(|x| x.as_i64()), a.get("track_number").and_then(|x| x.as_i64())) {
                    (Some(disc), Some(n)) if disc > 1 => format!("{title} #{disc}-{n}"),
                    (_, Some(n)) => format!("{title} #{n}"),
                    _ => title,
                }
            })
            .collect(),
        ("relations", Some(items)) => items
            .iter()
            .map(|r| {
                let kind = r.get("kind").and_then(|k| k.as_str()).unwrap_or("?").replace('_', " ");
                let title = id(r, "track_id").and_then(|t| catalog.track_index(t)).map(|i| catalog.tracks[i as usize].title.clone()).unwrap_or_else(|| "a track".into());
                format!("{kind} “{title}”")
            })
            .collect(),
        (_, Some(items)) => items.iter().map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).collect(),
        _ => return value.as_str().map(|s| format!("“{s}”")).unwrap_or_else(|| value.to_string()),
    };
    if list.is_empty() { "none".into() } else { list.join(", ") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::model;

    fn catalog() -> Catalog {
        let body = serde_json::json!({
            "format": 3,
            "tracks": {"columns": ["id", "title", "artist_display", "credits", "albums", "duration_ms", "file_id", "cover_id", "cover_key",
                                    "release_date", "year", "status", "genres", "created_at", "is_restricted", "codec", "groupings"], "rows": []},
            "albums": {"columns": ["id", "title", "kind", "artist_display", "credits", "release_date", "year", "cover_id", "cover_key", "created_at", "track_count", "duration_ms", "groupings"],
                       "rows": [["00000000-0000-7000-8000-0000000000b1", "LP", "album", "Ann", [], null, null, null, null, "", 1, 1]]},
            "artists": {"columns": ["id", "name", "disambiguation", "aliases", "track_count", "album_count"],
                        "rows": [["00000000-0000-7000-8000-0000000000a1", "Ann", "", [], 1, 1], ["00000000-0000-7000-8000-0000000000a2", "Bo", "", [], 1, 0]]},
            "annotations": {"columns": ["kind", "id", "play_count", "last_played_at", "rating", "starred_at"], "rows": []},
            "issues": {"columns": ["track_id", "flags"], "rows": []},
            "groupings": {"columns": ["id", "kind", "name", "track_count", "album_count", "artists"], "rows": []},
            "playlists": []
        });
        Catalog::build(model::parse(body.to_string().as_bytes()).unwrap())
    }

    #[test]
    fn changes_in_words() {
        let c = catalog();
        let before = serde_json::json!({ "title": "Song", "credits": [{ "artist_id": "00000000-0000-7000-8000-0000000000a1", "role": "primary" }] });
        let after = serde_json::json!({
            "title": "Song (Remix)", "genres": ["rap"],
            "credits": [{ "artist_id": "00000000-0000-7000-8000-0000000000a1", "role": "primary" },
                        { "artist_id": "00000000-0000-7000-8000-0000000000a2", "role": "featured" }],
        });
        assert_eq!(
            changes(Some(&before), &after, &c),
            ["title: “Song” → “Song (Remix)”", "genres: none → rap", "artists: Ann → Ann, Bo (featured)"]
        );
        let facts = serde_json::json!({ "status": "released", "albums": [{ "album_id": "00000000-0000-7000-8000-0000000000b1", "disc_number": 1, "track_number": 3 }] });
        let moved = serde_json::json!({ "status": "released", "albums": [] });
        assert_eq!(changes(Some(&facts), &moved, &c), ["albums: “LP” #3 → none"]);
        assert_eq!(changes(None, &after, &c), ["the first one"]);
        assert_eq!(changes(Some(&after), &after, &c), ["nothing different"]);
    }
}
