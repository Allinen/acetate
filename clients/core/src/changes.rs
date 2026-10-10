//! The changelist (`GET /api/changes`): the newest revisions of tracks, albums and
//! artists together, each with the one before it, so the change reads in words
//! (`editor::diff::changes_named`).

use std::collections::HashMap;

use serde::Deserialize;
use uuid::Uuid;

/// Changes asked for at a time.
pub const PAGE: usize = 50;

#[derive(Debug, Clone, Deserialize)]
pub struct Change {
    pub id: Uuid,
    /// "track", "album" or "artist".
    pub entity: String,
    pub entity_id: Uuid,
    /// What it's called now.
    pub name: String,
    /// A track's: "internal" (a version's metadata) or "external" (status, albums…).
    #[serde(default)]
    pub kind: String,
    pub created_at: String,
    pub author: Option<String>,
    /// "import", "edit", "merge", "restore"…
    pub source: String,
    #[serde(default)]
    pub reason: String,
    pub data: serde_json::Value,
    /// The revision before it (of the same kind and version); none for the first.
    pub before: Option<serde_json::Value>,
}

impl Change {
    /// What was changed, in a few words ("metadata", "status and albums", "artist").
    pub fn what(&self) -> &str {
        match (self.entity.as_str(), self.kind.as_str()) {
            ("track", "external") => "status and albums",
            ("track", _) => "track metadata",
            ("album", _) => "album",
            _ => "artist",
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Changes {
    pub changes: Vec<Change>,
    /// The names of the aliases the track changes credit, by id.
    #[serde(default)]
    pub alias_names: HashMap<Uuid, String>,
}

/// The request for a page of changes, after `before` (the last one shown).
pub fn path(before: Option<Uuid>, imports: bool) -> String {
    let before = before.map(|id| format!("&before={id}")).unwrap_or_default();
    format!("/api/changes?limit={PAGE}&imports={imports}{before}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_is_asked_for_after_the_last_one() {
        let id = Uuid::nil();
        assert_eq!(path(None, false), "/api/changes?limit=50&imports=false");
        assert_eq!(path(Some(id), true), format!("/api/changes?limit=50&imports=true&before={id}"));
    }

    #[test]
    fn what_the_server_sends_is_read() {
        let id = Uuid::nil();
        let body = serde_json::json!({
            "changes": [{
                "id": id, "entity": "track", "entity_id": id, "name": "Song", "kind": "external", "variant_id": null,
                "created_at": "2026-10-10T12:00:00Z", "author": "ann", "source": "edit", "reason": "", "data": {"status": "released"}, "before": null
            }],
            "alias_names": {}
        });
        let changes: Changes = serde_json::from_value(body).unwrap();
        assert_eq!(changes.changes[0].what(), "status and albums");
    }
}
