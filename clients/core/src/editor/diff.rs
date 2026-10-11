//! What a revision changed, in words (the history's lines, the editor's preview).

use std::collections::HashMap;

use serde_json::Value;
use uuid::Uuid;

use crate::catalog::Catalog;

/// What `after` changed from `before` (None: it's the first), in words.
#[cfg(test)]
pub fn changes(before: Option<&Value>, after: &Value, catalog: &Catalog) -> Vec<String> {
    changes_named(before, after, catalog, &HashMap::new())
}

/// Like `changes`, with names for ids not in the library (yet): a bulk edit's preview of
/// artists and albums it would make, or the aliases a track's history credits.
pub fn changes_named(before: Option<&Value>, after: &Value, catalog: &Catalog, names: &HashMap<Uuid, String>) -> Vec<String> {
    let Some(before) = before else { return vec!["the first one".into()] };
    let fields: [(&str, &str); 12] = [
        ("title", "title"),
        ("name", "name"),
        ("kind", "kind"),
        ("disambiguation", "tells apart"),
        ("aliases", "other names"),
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
                // Credited under an alias: which one.
                let name = match id(c, "alias_id").and_then(|a| names.get(&a)) {
                    Some(alias) => format!("{name} as “{alias}”"),
                    None => name,
                };
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
                    .and_then(|x| catalog.album(x).map(|x| format!("“{}”", x.title())).or_else(|| names.get(&x).map(|n| format!("“{n}” (new)"))))
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
                let title = id(r, "track_id").and_then(|t| catalog.track_index(t)).map(|i| catalog.track_at(i).title().to_string()).unwrap_or_else(|| "a track".into());
                format!("{kind} “{title}”")
            })
            .collect(),
        ("aliases", Some(items)) => items.iter().map(|a| a.get("name").and_then(|n| n.as_str()).map(|n| format!("“{n}”")).unwrap_or_else(|| a.to_string())).collect(),
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
            "format": crate::catalog::model::FORMAT,
            "tracks": {"columns": ["id", "title", "artist_display", "credits", "albums", "duration_ms", "file_id", "cover_id", "cover_key",
                                    "release_date", "year", "status", "genres", "created_at", "is_restricted", "codec", "groupings", "platforms", "original_filename", "bitrate", "sample_rate", "sha256"], "rows": []},
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
        // An album's and an artist's fields too.
        let album = serde_json::json!({ "title": "LP", "kind": "album" });
        assert_eq!(changes(Some(&album), &serde_json::json!({ "title": "LP", "kind": "ep", "release_date": "2019" }), &c), ["kind: “album” → “ep”", "released: none → “2019”"]);
        let artist = serde_json::json!({ "name": "Ann" });
        let named = serde_json::json!({ "name": "Ann", "disambiguation": "UK", "aliases": [{ "id": "00000000-0000-7000-8000-0000000000c1", "name": "Annie" }] });
        assert_eq!(changes(Some(&artist), &named, &c), ["tells apart: none → “UK”", "other names: none → “Annie”"]);
        assert_eq!(changes(Some(&after), &after, &c), ["nothing different"]);
    }
}
