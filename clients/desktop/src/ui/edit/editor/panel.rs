//! One track's fields as rows (the editor's state panel, with one track): each with its
//! value as the staged changes leave it (and what it was, where they change it), and the
//! review flags that are about it, now and after (from the server's preview). The keys on
//! a row edit it through the line (see `TrackEditor::row_key`): the line is the one way
//! things change.

use std::collections::HashMap;

use uuid::Uuid;

use super::engine::Ready;
use crate::catalog::Catalog;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RowKind {
    Title,
    /// A credit (its place in the credits as they'll be).
    Credit(usize),
    AddArtist,
    Date,
    Status,
    Genres,
    /// An album it's on (its place among them).
    Album(usize),
    AddAlbum,
    Related(usize),
    AddRelated,
    Comment,
    /// Flags about nothing here (no cover, sources, fix later…).
    Other,
}

pub struct Row {
    pub kind: RowKind,
    /// Its key, as the label (and the cheat sheet).
    pub key: &'static str,
    pub value: String,
    /// What it was, if the staged changes change it.
    pub was: Option<String>,
    /// Its flags: (kind, state).
    pub flags: Vec<(String, Flag)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Flag {
    /// Open now; the preview hasn't said yet.
    Open,
    /// Open now, gone after.
    Fixed,
    /// Open now and after.
    Still,
    /// Not open now, after: yes.
    New,
}

/// Which row a flag kind is about.
pub fn row_of(kind: &str) -> RowKind {
    match kind {
        "missing_title" | "credits_in_title" => RowKind::Title,
        "missing_artists" | "ambiguous_artist" | "merged_artist" => RowKind::AddArtist,
        "missing_album" => RowKind::AddAlbum,
        "missing_date" => RowKind::Date,
        _ => RowKind::Other,
    }
}

/// The line that starts fixing a flag (Review puts the cursor there), if a line does.
pub fn line_for(kind: &str, title: &str) -> Option<String> {
    match kind {
        "missing_title" => Some("title:".into()),
        // The title as it is, to take the credits out of.
        "credits_in_title" => Some(format!("title:{}", super::parse::quoted(title))),
        "missing_artists" => Some("artist:".into()),
        "missing_album" => Some("album:".into()),
        "missing_date" => Some("date:".into()),
        _ => None,
    }
}

/// The rows for one track's plan. `now`: its open flags; `after`: the preview's (None:
/// not there yet).
pub fn rows(r: &Ready, catalog: &Catalog, names: &HashMap<Uuid, String>, now: &[String], after: Option<&[String]>) -> Vec<Row> {
    let artist = |id: Uuid| catalog.artist(id).map(|a| a.name.clone()).or_else(|| names.get(&id).map(|n| format!("{n} (new)"))).unwrap_or_else(|| "(an artist)".into());
    let album = |id: Uuid| catalog.album(id).map(|a| a.title.clone()).or_else(|| names.get(&id).map(|n| format!("{n} (new)"))).unwrap_or_else(|| "(an album)".into());
    let track = |id: Uuid| catalog.track_index(id).map(|i| catalog.tracks[i as usize].title.clone()).unwrap_or_else(|| "(a track)".into());
    let credit = |c: &super::model::Credit| if c.role == "primary" { artist(c.artist_id) } else { format!("{} ({})", artist(c.artist_id), c.role) };
    let numbers = |m: &super::model::Membership| match (m.disc_number, m.track_number) {
        (Some(d), Some(n)) if d > 1 => format!(" #{d}-{n}"),
        (_, Some(n)) => format!(" #{n}"),
        _ => String::new(),
    };
    let changed = |before: String, after: String| -> (String, Option<String>) { if before == after { (after, None) } else { (after, Some(before)) } };

    let mut rows = Vec::new();
    let mut push = |kind: RowKind, key: &'static str, (value, was): (String, Option<String>)| rows.push(Row { kind, key, value, was, flags: Vec::new() });
    push(RowKind::Title, "title", changed(r.base.meta.title.clone(), r.meta.title.clone()));
    let before_credits: Vec<String> = r.base.meta.credits.iter().map(credit).collect();
    for (n, c) in r.meta.credits.iter().enumerate() {
        let text = credit(c);
        let alias = c.alias_id.map(|_| " (under another name)").unwrap_or("");
        let was = (!before_credits.contains(&text)).then(|| "new".to_string());
        push(RowKind::Credit(n), "artist", (format!("{text}{alias}"), was));
    }
    let removed: Vec<&String> = before_credits.iter().filter(|b| !r.meta.credits.iter().any(|c| credit(c) == **b)).collect();
    let add = if removed.is_empty() { String::new() } else { format!("(taken off: {})", removed.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")) };
    push(RowKind::AddArtist, "artist", (if r.meta.credits.is_empty() && add.is_empty() { "none".into() } else { add }, None));
    push(RowKind::Date, "date", changed(r.base.meta.release_date.clone().unwrap_or_default(), r.meta.release_date.clone().unwrap_or_default()));
    push(RowKind::Status, "status", changed(r.base.facts.status.clone(), r.facts.status.clone()));
    push(RowKind::Genres, "genre", changed(r.base.meta.genres.join(", "), r.meta.genres.join(", ")));
    let before_albums: Vec<String> = r.base.facts.albums.iter().map(|m| format!("{}{}", album(m.album_id), numbers(m))).collect();
    for (n, m) in r.facts.albums.iter().enumerate() {
        let text = format!("{}{}", album(m.album_id), numbers(m));
        let was = (!before_albums.contains(&text)).then(|| "new".to_string());
        push(RowKind::Album(n), "album", (text, was));
    }
    let gone: Vec<&String> = before_albums.iter().filter(|b| !r.facts.albums.iter().any(|m| format!("{}{}", album(m.album_id), numbers(m)) == **b)).collect();
    let add = if gone.is_empty() { String::new() } else { format!("(taken off: {})", gone.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")) };
    push(RowKind::AddAlbum, "album", (if r.facts.albums.is_empty() && add.is_empty() { "none".into() } else { add }, None));
    for (n, rel) in r.facts.relations.iter().enumerate() {
        let text = format!("{} “{}”", rel.kind.replace('_', " "), track(rel.track_id));
        let was = (!r.base.facts.relations.contains(rel)).then(|| "new".to_string());
        push(RowKind::Related(n), "related", (text, was));
    }
    push(RowKind::AddRelated, "related", (String::new(), None));
    push(RowKind::Comment, "comment", changed(r.base.meta.comment.clone().unwrap_or_default(), r.meta.comment.clone().unwrap_or_default()));

    // The flags, on their rows.
    let mut flags: Vec<(String, Flag)> = now
        .iter()
        .map(|k| {
            let state = match after {
                None => Flag::Open,
                Some(after) if after.contains(k) => Flag::Still,
                Some(_) => Flag::Fixed,
            };
            (k.clone(), state)
        })
        .collect();
    if let Some(after) = after {
        flags.extend(after.iter().filter(|k| !now.contains(k)).map(|k| (k.clone(), Flag::New)));
    }
    let mut other = Vec::new();
    for (kind, state) in flags {
        let place = row_of(&kind);
        match rows.iter_mut().find(|r| r.kind == place) {
            Some(row) if place != RowKind::Other => row.flags.push((kind, state)),
            _ => other.push((kind, state)),
        }
    }
    if !other.is_empty() {
        rows.push(Row { kind: RowKind::Other, key: "flags", value: String::new(), was: None, flags: other });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::edit::editor::model::{Base, Credit, Facts, Meta};

    fn catalog() -> Catalog {
        let body = serde_json::json!({
            "format": 8,
            "tracks": {"columns": ["id", "title", "artist_display", "credits", "albums", "duration_ms", "file_id", "cover_id", "cover_key",
                                    "release_date", "year", "status", "genres", "created_at", "is_restricted", "codec", "groupings", "platforms", "original_filename", "bitrate", "sample_rate"], "rows": []},
            "albums": {"columns": ["id", "title", "kind", "artist_display", "credits", "release_date", "year", "cover_id", "cover_key", "created_at", "track_count", "duration_ms", "groupings"], "rows": []},
            "artists": {"columns": ["id", "name", "disambiguation", "aliases", "track_count", "album_count"],
                        "rows": [["00000000-0000-7000-8000-0000000000a1", "Ann", "", [], 1, 0], ["00000000-0000-7000-8000-0000000000a2", "Bo", "", [], 1, 0]]},
            "annotations": {"columns": ["kind", "id", "play_count", "last_played_at", "rating", "starred_at"], "rows": []},
            "issues": {"columns": ["track_id", "flags"], "rows": []},
            "groupings": {"columns": ["id", "kind", "name", "track_count", "album_count", "artists"], "rows": []},
            "playlists": []
        });
        Catalog::build(crate::catalog::model::parse(body.to_string().as_bytes()).unwrap())
    }

    fn credit(id: &str, role: &str) -> Credit {
        Credit { artist_id: id.parse().unwrap(), role: role.into(), alias_id: None }
    }

    #[test]
    fn rows_say_what_changes_and_carry_their_flags() {
        let (ann, bo) = ("00000000-0000-7000-8000-0000000000a1", "00000000-0000-7000-8000-0000000000a2");
        let base = Base {
            variant: Uuid::from_u128(1),
            variant_base: Uuid::from_u128(2),
            meta: Meta { title: "Song (feat. Bo)".into(), credits: vec![credit(ann, "primary")], ..Default::default() },
            facts_base: Some(Uuid::from_u128(3)),
            facts: Facts { status: "released".into(), albums: vec![], relations: vec![] },
        };
        let meta = Meta { title: "Song".into(), credits: vec![credit(ann, "primary"), credit(bo, "featured")], ..Default::default() };
        let ready = Ready { position: 0, base: base.clone(), meta, facts: base.facts.clone(), lines: vec![], joins: vec![] };
        let now = ["credits_in_title".to_string(), "missing_album".to_string(), "missing_cover".to_string()];
        let after = ["missing_album".to_string(), "missing_cover".to_string()];
        let rows = rows(&ready, &catalog(), &HashMap::new(), &now, Some(&after));

        let title = rows.iter().find(|r| r.kind == RowKind::Title).unwrap();
        assert_eq!((title.value.as_str(), title.was.as_deref()), ("Song", Some("Song (feat. Bo)")));
        assert_eq!(title.flags, [("credits_in_title".to_string(), Flag::Fixed)]);
        let credits: Vec<(&str, bool)> = rows.iter().filter(|r| matches!(r.kind, RowKind::Credit(_))).map(|r| (r.value.as_str(), r.was.is_some())).collect();
        assert_eq!(credits, [("Ann", false), ("Bo (featured)", true)]);
        let albums = rows.iter().find(|r| r.kind == RowKind::AddAlbum).unwrap();
        assert_eq!(albums.flags, [("missing_album".to_string(), Flag::Still)]);
        // A flag about nothing here: on its own row, last.
        let other = rows.last().unwrap();
        assert_eq!((other.kind, other.flags.clone()), (RowKind::Other, vec![("missing_cover".to_string(), Flag::Still)]));
        // Before the preview: just open.
        let rows = super::rows(&ready, &catalog(), &HashMap::new(), &now, None);
        assert_eq!(rows[0].flags, [("credits_in_title".to_string(), Flag::Open)]);
    }

    #[test]
    fn the_line_that_starts_fixing_a_flag() {
        assert_eq!(line_for("missing_artists", "x").as_deref(), Some("artist:"));
        assert_eq!(line_for("credits_in_title", "Song (feat. Bo)").as_deref(), Some("title:Song (feat. Bo)"));
        assert_eq!(line_for("credits_in_title", "No: Way").as_deref(), Some("title:\"No: Way\""));
        assert_eq!(line_for("missing_cover", "x"), None);
    }
}
