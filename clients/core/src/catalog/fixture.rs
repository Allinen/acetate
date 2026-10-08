//! A small catalog for tests that need every kind of thing in it: credits with an alias,
//! a track on two albums, a grouping, an extra, annotations, issues, playlists.

use super::index::Catalog;
use super::model;

pub fn sample_json() -> String {
    serde_json::json!({
        "format": 8,
        "version": "7:7:",
        "tracks": {"columns": ["id", "title", "artist_display", "credits", "albums", "duration_ms", "file_id", "cover_id", "cover_key",
                                "release_date", "year", "status", "genres", "created_at", "is_restricted", "codec", "groupings", "platforms", "original_filename", "bitrate", "sample_rate"],
                   "rows": [
            ["00000000-0000-7000-8000-000000000001", "Second", "Ann", [["00000000-0000-7000-8000-0000000000a1", "primary", null]],
             [["00000000-0000-7000-8000-0000000000b1", 1, 2]], 1000, "00000000-0000-7000-8000-0000000000f1", null, null, "2020", 2020, "released", ["house", "ambient"], "", false, "flac",
             ["00000000-0000-7000-8000-0000000000d1"], ["youtube"], "second_master.wav", 900, 44100],
            ["00000000-0000-7000-8000-000000000002", "First", "Ann feat. Bo",
             [["00000000-0000-7000-8000-0000000000a1", "primary", null], ["00000000-0000-7000-8000-0000000000a2", "featured", "B.O."]],
             [["00000000-0000-7000-8000-0000000000b1", 1, 1]], 2000, "00000000-0000-7000-8000-0000000000f2", "00000000-0000-7000-8000-0000000000c1", "00112233445566778899", "2021", 2021, "released", [], "", true, "flac"],
            ["00000000-0000-7000-8000-000000000003", "The Third \u{c4}\u{d6}", "Bo", [["00000000-0000-7000-8000-0000000000a2", "primary", null], ["00000000-0000-7000-8000-0000000000a3", "producer", null]],
             [["00000000-0000-7000-8000-0000000000b2", null, 1], ["00000000-0000-7000-8000-0000000000b1", 1, 3]], 500, "00000000-0000-7000-8000-0000000000f3", null, null, null, null, "released", [], "", false, null]
        ]},
        "albums": {"columns": ["id", "title", "kind", "artist_display", "credits", "release_date", "year", "cover_id", "cover_key", "created_at", "track_count", "duration_ms", "groupings"],
                   "rows": [
            ["00000000-0000-7000-8000-0000000000b1", "A", "album", "Ann", [["00000000-0000-7000-8000-0000000000a1", "primary", null]], "2021", 2021, "00000000-0000-7000-8000-0000000000c1", "00112233445566778899", "", 3, 3500, ["00000000-0000-7000-8000-0000000000d1"]],
            ["00000000-0000-7000-8000-0000000000b2", "B", "single", "Bo", [["00000000-0000-7000-8000-0000000000a2", "primary", "B.O."]], null, null, null, null, "", 1, 500]
        ]},
        "artists": {"columns": ["id", "name", "disambiguation", "aliases", "track_count", "album_count"],
                    "rows": [
            ["00000000-0000-7000-8000-0000000000a1", "Ann", "", [], 2, 1],
            ["00000000-0000-7000-8000-0000000000a2", "Bo", "UK", ["B.O."], 2, 1],
            ["00000000-0000-7000-8000-0000000000a3", "Cy", "", [], 1, 0]
        ]},
        "annotations": {"columns": ["kind", "id", "play_count", "last_played_at", "rating", "starred_at"],
                        "rows": [["track", "00000000-0000-7000-8000-000000000002", 5, "2026-01-01", 8, "2026-02-02"]]},
        "issues": {"columns": ["track_id", "flags"], "rows": [
            ["00000000-0000-7000-8000-000000000001", [["00000000-0000-7000-8000-0000000000e1", "fix_later"], ["00000000-0000-7000-8000-0000000000e2", "missing_cover"]]]
        ]},
        "groupings": {"columns": ["id", "kind", "name", "track_count", "album_count", "artists"], "rows": [
            ["00000000-0000-7000-8000-0000000000d1", "era", "Early", 1, 1, ["00000000-0000-7000-8000-0000000000a1"]]
        ]},
        "extras": {"columns": ["id", "name", "kind", "is_restricted", "hidden", "links", "files", "first_file", "duration_ms", "video", "thumb_key"], "rows": [
            ["00000000-0000-7000-8000-0000000000e9", "Second (vocals)", "vocals", false, false, [["track", "00000000-0000-7000-8000-000000000001"]], 1, "00000000-0000-7000-8000-0000000000ea", 1000, null, null]
        ]},
        "playlists": [{"id": "00000000-0000-7000-8000-0000000000f9", "name": "Mine", "description": "", "public": false, "owner": "ann", "mine": true, "entries": 2, "duration_ms": 3000}]
    })
    .to_string()
}

pub fn sample() -> Catalog {
    Catalog::build(model::parse(sample_json().as_bytes()).unwrap())
}
