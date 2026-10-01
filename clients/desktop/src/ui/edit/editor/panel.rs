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
    /// No artists (there to add one).
    AddArtist,
    Date,
    Status,
    Genres,
    /// An album it's on (its place among them).
    Album(usize),
    /// On no album (there to add one).
    AddAlbum,
    Related(usize),
    /// No related tracks (there to add one).
    AddRelated,
    Comment,
    /// Something the staged changes take off (shown struck through).
    Removed,
    /// Flags about nothing here (no cover, sources, fix later…).
    Other,
}

pub struct Row {
    pub kind: RowKind,
    /// Its key, as the label (and the cheat sheet).
    pub key: &'static str,
    pub value: String,
    /// What it was, if the staged changes change it ("new": it wasn't there).
    pub was: Option<String>,
    /// Its flags: (kind, state).
    pub flags: Vec<(String, Flag)>,
    /// A credit's artist.
    pub artist: Option<Uuid>,
    /// An ambiguous artist credited here: the flag and its candidates (to choose from).
    pub choose: Option<(Uuid, Vec<Uuid>)>,
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

/// What the rows need to know besides the plan.
pub struct Know<'a> {
    pub catalog: &'a Catalog,
    /// The new artists' and albums' names (stand-ins).
    pub names: &'a HashMap<Uuid, String>,
    /// Aliases' names.
    pub aliases: &'a HashMap<Uuid, String>,
    /// Open ambiguous-artist flags: (the flag, its candidates).
    pub ambiguous: &'a [(Uuid, Vec<Uuid>)],
}

/// The rows for one track's plan. `now`: its open flags; `after`: the preview's (None:
/// not there yet).
pub fn rows(r: &Ready, know: &Know, now: &[String], after: Option<&[String]>) -> Vec<Row> {
    let catalog = know.catalog;
    let artist = |id: Uuid| catalog.artist(id).map(|a| a.name.clone()).or_else(|| know.names.get(&id).map(|n| format!("{n} (new)"))).unwrap_or_else(|| "(an artist)".into());
    let album = |id: Uuid| catalog.album(id).map(|a| a.title.clone()).or_else(|| know.names.get(&id).map(|n| format!("{n} (new)"))).unwrap_or_else(|| "(an album)".into());
    let track = |id: Uuid| catalog.track_index(id).map(|i| catalog.tracks[i as usize].title.clone()).unwrap_or_else(|| "(a track)".into());
    // "Ann", "Ann as “Annie”", "Bo (featured)".
    let credit = |c: &super::model::Credit| {
        let mut text = artist(c.artist_id);
        if let Some(alias) = c.alias_id {
            let name = know.aliases.get(&alias).map(|n| format!("“{n}”")).unwrap_or_else(|| "another of its names".into());
            text = format!("{text} as {name}");
        }
        if c.role != "primary" {
            text = format!("{text} ({})", c.role);
        }
        text
    };
    let membership = |m: &super::model::Membership| {
        let numbers = match (m.disc_number, m.track_number) {
            (Some(d), Some(n)) if d > 1 => format!(" #{d}-{n}"),
            (_, Some(n)) => format!(" #{n}"),
            _ => String::new(),
        };
        format!("{}{numbers}", album(m.album_id))
    };
    let relation = |rel: &super::model::Relation| format!("{} “{}”", rel.kind.replace('_', " "), track(rel.track_id));
    let changed = |before: String, after: String| -> (String, Option<String>) { if before == after { (after, None) } else { (after, Some(before)) } };
    let row = |kind: RowKind, key: &'static str, (value, was): (String, Option<String>)| Row { kind, key, value, was, flags: Vec::new(), artist: None, choose: None };

    let mut rows = Vec::new();
    rows.push(row(RowKind::Title, "title", changed(r.base.meta.title.clone(), r.meta.title.clone())));

    // A list: what it will hold (new ones marked), what it takes off (struck through), or
    // a row to add one to an empty list.
    // A credit is the same one by its artist and role (its name may change).
    let same = |a: &super::model::Credit, b: &super::model::Credit| a.artist_id == b.artist_id && a.role == b.role;
    for (n, c) in r.meta.credits.iter().enumerate() {
        let text = credit(c);
        let was = match r.base.meta.credits.iter().find(|b| same(b, c)) {
            Some(b) => (b != c).then(|| credit(b)),
            None => Some("new".to_string()),
        };
        let mut x = row(RowKind::Credit(n), "artist", (text, was));
        x.artist = Some(c.artist_id);
        x.choose = know.ambiguous.iter().find(|(_, candidates)| candidates.contains(&c.artist_id)).cloned();
        rows.push(x);
    }
    for gone in r.base.meta.credits.iter().filter(|b| !r.meta.credits.iter().any(|c| same(b, c))) {
        rows.push(row(RowKind::Removed, "artist", (String::new(), Some(credit(gone)))));
    }
    if r.meta.credits.is_empty() {
        rows.push(row(RowKind::AddArtist, "artist", ("none".into(), None)));
    }
    rows.push(row(RowKind::Date, "date", changed(r.base.meta.release_date.clone().unwrap_or_default(), r.meta.release_date.clone().unwrap_or_default())));
    rows.push(row(RowKind::Status, "status", changed(r.base.facts.status.clone(), r.facts.status.clone())));
    rows.push(row(RowKind::Genres, "genre", changed(r.base.meta.genres.join(", "), r.meta.genres.join(", "))));
    let before: Vec<String> = r.base.facts.albums.iter().map(membership).collect();
    let after_list: Vec<String> = r.facts.albums.iter().map(membership).collect();
    for (n, text) in after_list.iter().enumerate() {
        let was = (!before.contains(text)).then(|| "new".to_string());
        rows.push(row(RowKind::Album(n), "album", (text.clone(), was)));
    }
    for gone in before.iter().filter(|b| !after_list.contains(b)) {
        rows.push(row(RowKind::Removed, "album", (String::new(), Some(gone.clone()))));
    }
    if r.facts.albums.is_empty() {
        rows.push(row(RowKind::AddAlbum, "album", ("none".into(), None)));
    }
    let before: Vec<String> = r.base.facts.relations.iter().map(relation).collect();
    let after_list: Vec<String> = r.facts.relations.iter().map(relation).collect();
    for (n, text) in after_list.iter().enumerate() {
        let was = (!before.contains(text)).then(|| "new".to_string());
        rows.push(row(RowKind::Related(n), "related", (text.clone(), was)));
    }
    for gone in before.iter().filter(|b| !after_list.contains(b)) {
        rows.push(row(RowKind::Removed, "related", (String::new(), Some(gone.clone()))));
    }
    if r.facts.relations.is_empty() {
        rows.push(row(RowKind::AddRelated, "related", ("none".into(), None)));
    }
    rows.push(row(RowKind::Comment, "comment", changed(r.base.meta.comment.clone().unwrap_or_default(), r.meta.comment.clone().unwrap_or_default())));

    // The flags, on the rows they're about.
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
    let first = |rows: &[Row], f: &dyn Fn(RowKind) -> bool| rows.iter().position(|r| f(r.kind));
    let mut other = Vec::new();
    for (kind, state) in flags {
        let artists = |k: RowKind| matches!(k, RowKind::Credit(_) | RowKind::AddArtist);
        let at: Vec<usize> = match kind.as_str() {
            "missing_title" | "credits_in_title" => first(&rows, &|k| k == RowKind::Title).into_iter().collect(),
            "missing_date" => first(&rows, &|k| k == RowKind::Date).into_iter().collect(),
            "missing_album" => first(&rows, &|k| matches!(k, RowKind::Album(_) | RowKind::AddAlbum)).into_iter().collect(),
            // On the credit of the candidate it names (each, if several are), else on the artists.
            "ambiguous_artist" => {
                let on: Vec<usize> = rows.iter().enumerate().filter(|(_, r)| r.choose.is_some()).map(|(i, _)| i).collect();
                if on.is_empty() { first(&rows, &artists).into_iter().collect() } else { on }
            }
            "missing_artists" | "merged_artist" => first(&rows, &artists).into_iter().collect(),
            _ => Vec::new(),
        };
        for &i in &at {
            rows[i].flags.push((kind.clone(), state));
        }
        if at.is_empty() {
            other.push((kind, state));
        }
    }
    if !other.is_empty() {
        let n = other.len();
        let value = if n == 1 { "1 not about a field above".to_string() } else { format!("{n} not about a field above") };
        let mut x = row(RowKind::Other, "flags", (value, None));
        x.flags = other;
        rows.push(x);
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
        let ready = Ready { position: 0, base: base.clone(), meta, facts: base.facts.clone(), lines: vec![], joins: vec![], sync: false };
        let now = ["credits_in_title".to_string(), "missing_album".to_string(), "missing_cover".to_string()];
        let after = ["missing_album".to_string(), "missing_cover".to_string()];
        let catalog = catalog();
        let (names, aliases) = (HashMap::new(), HashMap::new());
        let know = Know { catalog: &catalog, names: &names, aliases: &aliases, ambiguous: &[] };
        let rows = rows(&ready, &know, &now, Some(&after));

        let title = rows.iter().find(|r| r.kind == RowKind::Title).unwrap();
        assert_eq!((title.value.as_str(), title.was.as_deref()), ("Song", Some("Song (feat. Bo)")));
        assert_eq!(title.flags, [("credits_in_title".to_string(), Flag::Fixed)]);
        let credits: Vec<(&str, bool)> = rows.iter().filter(|r| matches!(r.kind, RowKind::Credit(_))).map(|r| (r.value.as_str(), r.was.is_some())).collect();
        assert_eq!(credits, [("Ann", false), ("Bo (featured)", true)]);
        // No "add" row where there are some; one where there are none.
        assert!(!rows.iter().any(|r| r.kind == RowKind::AddArtist));
        let albums = rows.iter().find(|r| r.kind == RowKind::AddAlbum).unwrap();
        assert_eq!(albums.flags, [("missing_album".to_string(), Flag::Still)]);
        // A flag about nothing here: on its own row, last.
        let other = rows.last().unwrap();
        assert_eq!((other.kind, other.flags.clone()), (RowKind::Other, vec![("missing_cover".to_string(), Flag::Still)]));
        // Before the preview: just open.
        let rows = super::rows(&ready, &know, &now, None);
        assert_eq!(rows[0].flags, [("credits_in_title".to_string(), Flag::Open)]);
    }

    #[test]
    fn ambiguous_names_aliases_and_what_goes() {
        let (ann, bo) = ("00000000-0000-7000-8000-0000000000a1", "00000000-0000-7000-8000-0000000000a2");
        let base = Base {
            variant: Uuid::from_u128(1),
            variant_base: Uuid::from_u128(2),
            meta: Meta { title: "Song".into(), credits: vec![credit(ann, "primary"), credit(bo, "producer")], ..Default::default() },
            facts_base: Some(Uuid::from_u128(3)),
            facts: Facts { status: "released".into(), albums: vec![], relations: vec![] },
        };
        // Bo taken off; Ann credited under an alias.
        let alias = Uuid::from_u128(70);
        let meta = Meta { credits: vec![Credit { alias_id: Some(alias), ..credit(ann, "primary") }], ..base.meta.clone() };
        let ready = Ready { position: 0, base: base.clone(), meta, facts: base.facts.clone(), lines: vec![], joins: vec![], sync: false };
        let catalog = catalog();
        let names = HashMap::new();
        let aliases = HashMap::from([(alias, "Annie".to_string())]);
        let flag = Uuid::from_u128(80);
        let ambiguous = [(flag, vec![ann.parse().unwrap(), Uuid::from_u128(99)])];
        let know = Know { catalog: &catalog, names: &names, aliases: &aliases, ambiguous: &ambiguous };
        let rows = rows(&ready, &know, &["ambiguous_artist".to_string()], None);

        let ann_row = rows.iter().find(|r| r.kind == RowKind::Credit(0)).unwrap();
        assert_eq!(ann_row.value, "Ann as “Annie”");
        // The ambiguous flag is on the credit of the candidate, with what to choose from.
        assert_eq!(ann_row.flags, [("ambiguous_artist".to_string(), Flag::Open)]);
        assert_eq!(ann_row.choose.as_ref().map(|c| c.0), Some(flag));
        // Bo, taken off: its own row, struck through.
        let gone = rows.iter().find(|r| r.kind == RowKind::Removed).unwrap();
        assert_eq!((gone.key, gone.was.as_deref()), ("artist", Some("Bo (producer)")));
        assert!(!rows.iter().any(|r| r.kind == RowKind::Other));
    }

    #[test]
    fn the_line_that_starts_fixing_a_flag() {
        assert_eq!(line_for("missing_artists", "x").as_deref(), Some("artist:"));
        assert_eq!(line_for("credits_in_title", "Song (feat. Bo)").as_deref(), Some("title:Song (feat. Bo)"));
        assert_eq!(line_for("credits_in_title", "No: Way").as_deref(), Some("title:\"No: Way\""));
        assert_eq!(line_for("missing_cover", "x"), None);
    }
}
