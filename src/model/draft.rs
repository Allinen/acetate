//! A track made only in a preview: a file that didn't import, imported with the editor's
//! changes and rolled back (see `api::problems::import`). The client edits it like any
//! track, so its edit refers to the draft's ids: its variant and revisions, covers,
//! sources, the artists and album the import made. Those are gone once the preview is
//! rolled back, and importing it for real makes them again under new ids.
//!
//! So the preview answers a `Draft` with the track: every id in it, by where it is. The
//! client sends it back with its edit, and the server maps each of those ids to the one at
//! the same place in the track it has just made for real, then rewrites the edit with them.
//! Places only match if the import came out the same: the rest of the track (all but its
//! ids and times) is summed up in `shape`, and if that differs the client is told to
//! preview again.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    /// The ids in the track, by where they are in its JSON (`credits.0.artist_id`).
    pub ids: BTreeMap<String, Uuid>,
    /// The sha256 (hex) of the track's JSON without its ids and times.
    pub shape: String,
}

impl Draft {
    /// The draft of a track as the API answers it.
    pub fn of(track: &Value) -> Draft {
        let mut ids = BTreeMap::new();
        let shape = without_ids(track, &mut String::new(), &mut ids);
        let hash = Sha256::digest(shape.to_string().as_bytes());
        Draft { ids, shape: hex::encode(hash) }
    }

    /// Each of this draft's ids → the one at the same place in `made` (the same import,
    /// done again). Err: the import came out differently.
    pub fn map_to(&self, made: &Draft) -> Result<HashMap<Uuid, Uuid>, String> {
        let changed = || "the import came out differently than when it was previewed: preview it again".to_string();
        if self.shape != made.shape || self.ids.len() != made.ids.len() {
            return Err(changed());
        }
        let mut map = HashMap::new();
        for (place, id) in &self.ids {
            let Some(new) = made.ids.get(place) else { return Err(changed()) };
            // The same id in two places has to become the same id in both.
            if *map.entry(*id).or_insert(*new) != *new {
                return Err(changed());
            }
        }
        map.retain(|old, new| old != new);
        Ok(map)
    }
}

/// `value` with its ids (collected into `ids`, by place) and times left out.
fn without_ids(value: &Value, place: &mut String, ids: &mut BTreeMap<String, Uuid>) -> Value {
    match value {
        Value::String(s) => {
            if let Some(id) = as_id(s) {
                ids.insert(place.clone(), id);
                Value::String("<id>".into())
            } else if chrono::DateTime::parse_from_rfc3339(s).is_ok() {
                Value::String("<time>".into())
            } else {
                value.clone()
            }
        }
        // Keyed by ids (e.g. `alias_names`): those are in the track elsewhere too, where
        // they're used; what's here is read by them.
        Value::Object(map) if map.keys().any(|k| as_id(k).is_some()) => Value::String("<by id>".into()),
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, v) in map {
                let len = place.len();
                if !place.is_empty() {
                    place.push('.');
                }
                place.push_str(key);
                out.insert(key.clone(), without_ids(v, place, ids));
                place.truncate(len);
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let len = place.len();
                    if !place.is_empty() {
                        place.push('.');
                    }
                    place.push_str(&i.to_string());
                    let out = without_ids(v, place, ids);
                    place.truncate(len);
                    out
                })
                .collect(),
        ),
        _ => value.clone(),
    }
}

/// An id as the API writes them (hyphenated), not any 32 hex digits.
fn as_id(s: &str) -> Option<Uuid> {
    if s.len() == 36 { Uuid::parse_str(s).ok() } else { None }
}

/// Replaces the ids in `value` that are in `map` (an edit, its ids a draft's).
pub fn replace_ids(value: &mut Value, map: &HashMap<Uuid, Uuid>) {
    match value {
        Value::String(s) => {
            if let Some(new) = as_id(s).and_then(|id| map.get(&id)) {
                *s = new.to_string();
            }
        }
        Value::Object(map_) => map_.values_mut().for_each(|v| replace_ids(v, map)),
        Value::Array(items) => items.iter_mut().for_each(|v| replace_ids(v, map)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn track(ids: [u128; 3], title: &str) -> Value {
        json!({
            "id": id(ids[0]),
            "title": title,
            "created_at": "2026-10-03T10:00:00Z",
            "credits": [{ "artist_id": id(ids[1]), "role": "primary" }, { "artist_id": id(50), "role": "producer" }],
            "covers": [{ "id": id(ids[2]), "sha256": "ab" }],
            "alias_names": { id(ids[1]).to_string(): "Foo" },
        })
    }

    #[test]
    fn ids_map_by_place() {
        let preview = Draft::of(&track([1, 2, 3], "Song"));
        let mut made = track([11, 12, 13], "Song");
        made["created_at"] = json!("2026-10-04T11:00:00+02:00");
        let made = Draft::of(&made);
        let map = preview.map_to(&made).unwrap();
        // The producer is an artist that was there already: the same in both.
        assert_eq!(map, HashMap::from([(id(1), id(11)), (id(2), id(12)), (id(3), id(13))]));

        let mut edit = json!({ "track_id": id(1), "meta": { "data": { "credits": [{ "artist_id": id(2) }, { "artist_id": id(50) }] } } });
        replace_ids(&mut edit, &map);
        assert_eq!(edit, json!({ "track_id": id(11), "meta": { "data": { "credits": [{ "artist_id": id(12) }, { "artist_id": id(50) }] } } }));
    }

    #[test]
    fn a_different_import_is_refused() {
        let preview = Draft::of(&track([1, 2, 3], "Song"));
        assert!(preview.map_to(&Draft::of(&track([11, 12, 13], "Other"))).is_err());
        // One id in two places, two in the made one.
        let mut twice = track([1, 2, 3], "Song");
        twice["covers"][0]["id"] = json!(id(2));
        let mut apart = track([11, 12, 13], "Song");
        apart["covers"][0]["id"] = json!(id(14));
        assert!(Draft::of(&twice).map_to(&Draft::of(&apart)).is_err());
    }
}
