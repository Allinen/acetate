//! Folding a kept copy's deltas into its whole catalog, so the next start reads one file.
//!
//! Done on the text: rows stay as the server sent them (each one a slice of the body), only
//! their order and which are kept changes, so nothing here needs to know a row's fields
//! beyond its id. The result is a catalog body like the server's, that `model::parse`
//! reads, at the last delta's version.

use anyhow::{bail, Context};
use serde::Deserialize;
use serde_json::value::RawValue;
use uuid::Uuid;

use super::index::Rows;
use super::model::{self, Removed};

#[derive(Deserialize, Default)]
#[serde(default)]
struct Part<'a> {
    columns: Vec<String>,
    #[serde(borrow)]
    rows: Vec<&'a RawValue>,
}

#[derive(Deserialize)]
struct Doc<'a> {
    format: u32,
    version: Option<String>,
    #[serde(default)]
    removed: Removed,
    #[serde(borrow)]
    tracks: Part<'a>,
    #[serde(borrow)]
    albums: Part<'a>,
    #[serde(borrow)]
    artists: Part<'a>,
    #[serde(borrow)]
    annotations: Part<'a>,
    #[serde(borrow)]
    issues: Part<'a>,
    #[serde(borrow)]
    groupings: Part<'a>,
    #[serde(borrow, default)]
    extras: Part<'a>,
    #[serde(borrow)]
    playlists: &'a RawValue,
}

fn read(body: &[u8]) -> anyhow::Result<Doc<'_>> {
    let doc: Doc = serde_json::from_str(std::str::from_utf8(body)?)?;
    if doc.format != model::FORMAT {
        bail!("another format");
    }
    model::check_columns("tracks", &doc.tracks.columns, model::TRACK_COLUMNS)?;
    model::check_columns("albums", &doc.albums.columns, model::ALBUM_COLUMNS)?;
    model::check_columns("artists", &doc.artists.columns, model::ARTIST_COLUMNS)?;
    model::check_columns("annotations", &doc.annotations.columns, model::ANNOTATION_COLUMNS)?;
    model::check_columns("issues", &doc.issues.columns, model::ISSUE_COLUMNS)?;
    model::check_columns("groupings", &doc.groupings.columns, model::GROUPING_COLUMNS)?;
    if !doc.extras.columns.is_empty() {
        model::check_columns("extras", &doc.extras.columns, model::EXTRA_COLUMNS)?;
    }
    Ok(doc)
}

/// A row's id and its text.
type Row<'a> = (Uuid, &'a str);

/// A row whose first element is its id (rows are compact arrays).
pub(super) fn first_id(row: &RawValue) -> anyhow::Result<Row<'_>> {
    let text = row.get();
    let id = text
        .strip_prefix("[\"")
        .and_then(|rest| rest.get(..36))
        .and_then(|id| Uuid::parse_str(id).ok())
        .with_context(|| format!("a row that doesn't start with an id: {}", &text[..text.len().min(40)]))?;
    Ok((id, text))
}

fn rows<'a>(part: &Part<'a>) -> anyhow::Result<Vec<Row<'a>>> {
    part.rows.iter().map(|r| first_id(r)).collect()
}

/// An annotation's id is its second element (the first is its kind).
fn annotations<'a>(part: &Part<'a>) -> anyhow::Result<Vec<Row<'a>>> {
    part.rows
        .iter()
        .map(|r| {
            let model::Annotation { id, .. } = serde_json::from_str(r.get())?;
            Ok((id, r.get()))
        })
        .collect()
}

fn id(row: &Row) -> Uuid {
    row.0
}

fn write_part(out: &mut String, name: &str, columns: &[String], rows: impl Iterator<Item = String>) -> anyhow::Result<()> {
    out.push_str(&format!("\"{name}\":{{\"columns\":{},\"rows\":[", serde_json::to_string(columns)?));
    for (n, row) in rows.enumerate() {
        if n > 0 {
            out.push(',');
        }
        out.push_str(&row);
    }
    out.push_str("]},");
    Ok(())
}

/// The whole catalog `body` with each of `deltas` (a line each) applied in order.
pub fn fold(body: &[u8], deltas: &[u8]) -> anyhow::Result<Vec<u8>> {
    let base = read(body)?;
    let mut tracks = Rows::new(rows(&base.tracks)?, id);
    let mut albums = Rows::new(rows(&base.albums)?, id);
    let mut artists = Rows::new(rows(&base.artists)?, id);
    let mut extras = Rows::new(rows(&base.extras)?, id);
    let mut notes = Rows::new(annotations(&base.annotations)?, id);
    let mut issues = Rows::new(rows(&base.issues)?, id);
    let (mut version, mut groupings, mut playlists) = (base.version.clone(), &base.groupings, base.playlists);

    // Each delta's text lives until the end: the rows borrow from it.
    let mut docs = Vec::new();
    for line in deltas.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        // A half-written last line (a crash while saving): the rest is fine.
        match read(line) {
            Ok(doc) => docs.push(doc),
            Err(_) => break,
        }
    }
    for doc in &docs {
        let gone = &doc.removed;
        tracks.apply(rows(&doc.tracks)?, &gone.tracks, id);
        albums.apply(rows(&doc.albums)?, &gone.albums, id);
        artists.apply(rows(&doc.artists)?, &gone.artists, id);
        extras.apply(rows(&doc.extras)?, &gone.extras, id);
        notes.apply(annotations(&doc.annotations)?, &gone.annotations, id);
        // A track with no flags left has an empty list: it goes.
        let (mut changed, mut cleared) = (Vec::new(), gone.tracks.clone());
        for row in rows(&doc.issues)? {
            let model::Issues(_, flags) = serde_json::from_str(row.1)?;
            if flags.is_empty() {
                cleared.push(row.0);
            } else {
                changed.push(row);
            }
        }
        issues.apply(changed, &cleared, id);
        version = doc.version.clone().or(version);
        groupings = &doc.groupings;
        playlists = doc.playlists;
    }

    let mut out = String::with_capacity(body.len() + deltas.len() / 2);
    out.push_str(&format!("{{\"format\":{},\"version\":{},\"delta\":false,", model::FORMAT, serde_json::to_string(&version)?));
    fn texts(rows: Rows<Row>) -> impl Iterator<Item = String> {
        rows.finish().into_iter().map(|r| r.1.to_string())
    }
    write_part(&mut out, "tracks", &base.tracks.columns, texts(tracks))?;
    write_part(&mut out, "albums", &base.albums.columns, texts(albums))?;
    write_part(&mut out, "artists", &base.artists.columns, texts(artists))?;
    write_part(&mut out, "annotations", &base.annotations.columns, texts(notes))?;
    write_part(&mut out, "issues", &base.issues.columns, texts(issues))?;
    write_part(&mut out, "groupings", &groupings.columns, groupings.rows.iter().map(|r| r.get().to_string()))?;
    write_part(&mut out, "extras", &base.extras.columns, texts(extras))?;
    out.push_str(&format!("\"playlists\":{}}}", playlists.get()));
    Ok(out.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;

    fn t(n: u8) -> String {
        format!("00000000-0000-7000-8000-00000000000{n}")
    }

    fn track(n: u8, title: &str) -> serde_json::Value {
        serde_json::json!([t(n), title, "Ann", [], [], 1000, "00000000-0000-7000-8000-0000000000f9", null, null, null, null, "released", [], "", false, "flac"])
    }

    /// A catalog body (a delta if `version` says so by its rows) with the given parts.
    fn body(delta: bool, version: &str, tracks: Vec<serde_json::Value>, issues: Vec<serde_json::Value>, removed: serde_json::Value) -> Vec<u8> {
        let part = |columns: &[&str], rows: Vec<serde_json::Value>| serde_json::json!({"columns": columns, "rows": rows});
        serde_json::json!({
            "format": model::FORMAT, "version": version, "delta": delta, "removed": removed,
            "tracks": part(model::TRACK_COLUMNS, tracks),
            "albums": part(model::ALBUM_COLUMNS, vec![]),
            "artists": part(model::ARTIST_COLUMNS, vec![]),
            "annotations": part(model::ANNOTATION_COLUMNS, vec![]),
            "issues": part(model::ISSUE_COLUMNS, issues),
            "groupings": part(model::GROUPING_COLUMNS, vec![]),
            "extras": part(model::EXTRA_COLUMNS, vec![]),
            "playlists": [],
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn folding_gives_what_applying_the_deltas_does() {
        let flag = |n: u8| serde_json::json!([t(n), [["00000000-0000-7000-8000-0000000000e1", "fix_later"]]]);
        let base = body(false, "1:1:", vec![track(1, "One"), track(2, "Two"), track(3, "Three")], vec![flag(1), flag(2)], serde_json::json!({}));
        let first = body(true, "2:2:", vec![track(1, "One (edited)"), track(4, "Four")], vec![serde_json::json!([t(1), []])], serde_json::json!({"tracks": [t(3)]}));
        let second = body(true, "3:3:", vec![track(2, "Two (edited)")], vec![], serde_json::json!({}));
        let deltas = [first.as_slice(), b"\n", second.as_slice(), b"\n{\"half writ"].concat();

        let folded = fold(&base, &deltas).unwrap();
        let a = Catalog::build(model::parse(&folded).unwrap());

        // As a start loads it: the whole catalog's rows read as they're built, the deltas on top.
        let mut merger = super::super::index::Merger::from_base(model::parse_base(&base).unwrap()).unwrap();
        for delta in [&first, &second] {
            merger.add(model::parse(delta).unwrap());
        }
        let b = merger.build(None).unwrap();
        // As a refresh does it: each delta applied to the catalog shown.
        let mut c = Catalog::build(model::parse(&base).unwrap());
        for delta in [&first, &second] {
            c = c.apply(model::parse(delta).unwrap());
        }

        let titles = |c: &Catalog| {
            let mut titles: Vec<String> = c.tracks().map(|t| t.title().to_string()).collect();
            titles.sort();
            titles
        };
        assert_eq!(titles(&a), ["Four", "One (edited)", "Two (edited)"]);
        assert_eq!(titles(&a), titles(&b));
        assert_eq!(titles(&a), titles(&c));
        assert_eq!(c.version.as_deref(), Some("3:3:"));
        assert_eq!(a.version.as_deref(), Some("3:3:"));
        assert!(!a.issues.contains_key(&t(1).parse().unwrap()), "track 1's flags were all settled");
        // Folding what's folded changes nothing.
        assert_eq!(fold(&folded, b"").unwrap(), folded);
    }
}
