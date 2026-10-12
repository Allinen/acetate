//! An album's fields as rows in edit mode, as the track editor shows a track's: each field
//! as it will be once the staged changes are saved, with what it was struck through beside
//! it. The rows are only shown: every change is a command in the album's commandline
//! (`album_commands`), which the row keys fill in or stage (Enter, `r`, `a`, Alt+↑/↓,
//! Delete).

use eframe::egui::{self, RichText};
use uuid::Uuid;

use super::album_commands::{Credit, Edit};
use crate::ui::edit::credits::CreditRow;
use crate::ui::theme::PALETTE as P;
use acetate_core::editor::forms::AlbumData;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Field {
    Title,
    Kind,
    Date,
    /// An artist credited (its place in the credits as they will be).
    Credit(usize),
    /// No artists (there to add one).
    AddArtist,
    /// An artist the staged changes take off (shown struck through).
    Removed,
}

pub struct Row {
    pub kind: Field,
    /// Its key in the commandline.
    pub key: &'static str,
    pub value: String,
    /// What it was, if the staged changes change it ("new": it wasn't there).
    pub was: Option<String>,
    /// A credit's artist: None for a new one (it isn't made until the album is saved).
    pub artist: Option<Uuid>,
    /// A credit's artist's name, as the commandline names it, and its role.
    pub name: String,
    pub role: String,
}

/// An artist's credit in words: the artist, its role, and the name it's credited under.
fn credit_text(c: &CreditRow) -> String {
    match &c.alias {
        Some(alias) => format!("{} · {} · as {alias}", c.name, c.role),
        None => format!("{} · {}", c.name, c.role),
    }
}

/// One of the form's changes onto the album's data. (A credit already there changes; the
/// others are added. Changes that aren't the form's are left alone.)
pub fn apply(data: &mut AlbumData, edit: &Edit) {
    match edit {
        Edit::Title(title) => data.title = title.clone(),
        Edit::Kind(kind) => data.kind = kind.clone(),
        Edit::Date(date) => data.release_date = date.clone(),
        Edit::Credit(c) => credit_into(&mut data.credits, c.clone()),
        Edit::Uncredit(artist, role) => data.credits.retain(|r| r.artist_id != Some(*artist) || role.as_ref().is_some_and(|role| r.role != *role)),
        Edit::NoCredits => data.credits.clear(),
        Edit::Why(_) | Edit::Thing(_) | Edit::Cover(_) | Edit::Group(_) => {}
    }
}

/// A credit onto the rows, as the track editor does it: with a role, the artist's credit in
/// that role changes (its name, place), else it's a credit of its own (an artist can be
/// primary and producer); without one, the artist's credit in any role, else it's added
/// (as primary if there's none yet, else featured).
pub fn credit_into(rows: &mut Vec<CreditRow>, c: Credit) {
    let existing = c.artist.and_then(|id| match &c.role {
        Some(role) => rows.iter().position(|r| r.artist_id == Some(id) && r.role == *role),
        None => rows.iter().position(|r| r.artist_id == Some(id)),
    });
    let at = match existing {
        Some(at) => at,
        None => {
            let role = c.role.clone().unwrap_or_else(|| if rows.iter().any(|r| r.role == "primary") { "featured".into() } else { "primary".into() });
            rows.push(CreditRow { artist_id: c.artist, name: c.name, role, alias_id: None, alias: None });
            rows.len() - 1
        }
    };
    if let Some(alias) = c.alias {
        let row = &mut rows[at];
        row.alias_id = alias.as_ref().map(|a| a.0);
        row.alias = alias.map(|a| a.1);
    }
    if let Some(to) = c.at {
        let row = rows.remove(at);
        rows.insert(to.min(rows.len()), row);
    }
}

/// The rows: `base` as the album is, with `staged` (the form's changes, in order) on top.
pub fn rows<'a>(base: &AlbumData, staged: impl Iterator<Item = &'a Edit>) -> Vec<Row> {
    let mut after = base.clone();
    for edit in staged {
        apply(&mut after, edit);
    }
    let was = |before: &str, now: &str| (before != now).then(|| before.to_string());
    let mut rows = vec![
        Row { kind: Field::Title, key: "title", value: after.title.clone(), was: was(&base.title, &after.title), artist: None, name: String::new(), role: String::new() },
        Row { kind: Field::Kind, key: "kind", value: after.kind.clone(), was: was(&base.kind, &after.kind), artist: None, name: String::new(), role: String::new() },
        Row { kind: Field::Date, key: "date", value: after.release_date.clone(), was: was(&base.release_date, &after.release_date), artist: None, name: String::new(), role: String::new() },
    ];
    // A credit is its artist in a role (one artist can have several).
    let same = |a: &CreditRow, b: &CreditRow| a.artist_id.is_some() && a.artist_id == b.artist_id && a.role == b.role;
    for (n, credit) in after.credits.iter().enumerate() {
        let text = credit_text(credit);
        let was = match base.credits.iter().find(|b| same(b, credit)) {
            None => Some("new".to_string()),
            Some(b) => {
                let place = base.credits.iter().position(|x| same(x, b)).unwrap_or(n);
                if credit_text(b) != text {
                    Some(credit_text(b))
                } else {
                    (place != n).then(|| format!("place {}", place + 1))
                }
            }
        };
        rows.push(Row { kind: Field::Credit(n), key: "artist", value: text, was, artist: credit.artist_id, name: credit.name.clone(), role: credit.role.clone() });
    }
    if after.credits.is_empty() {
        rows.push(Row { kind: Field::AddArtist, key: "artist", value: String::new(), was: None, artist: None, name: String::new(), role: String::new() });
    }
    for gone in base.credits.iter().filter(|b| !after.credits.iter().any(|a| same(a, b))) {
        rows.push(Row { kind: Field::Removed, key: "artist", value: String::new(), was: Some(credit_text(gone)), artist: gone.artist_id, name: gone.name.clone(), role: gone.role.clone() });
    }
    rows
}

/// The rows, the one the keys are on (`selected`) lit. Returns the row clicked.
pub fn show(ui: &mut egui::Ui, rows: &[Row], selected: Option<usize>) -> Option<usize> {
    let mut clicked = None;
    egui::Grid::new("album-rows").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
        for (n, row) in rows.iter().enumerate() {
            let lit = selected == Some(n);
            let key = RichText::new(format!("{}:", row.key)).color(if lit { P.accent } else { P.faint });
            if ui.add(egui::Label::new(key).sense(egui::Sense::click())).clicked() {
                clicked = Some(n);
            }
            ui.horizontal_wrapped(|ui| {
                let mut value = if row.kind == Field::Removed {
                    RichText::new(row.was.clone().unwrap_or_default()).color(P.faint).strikethrough()
                } else {
                    let empty = row.value.is_empty();
                    let color = if row.was.is_some() { P.accent } else if empty { P.faint } else { P.text };
                    RichText::new(if empty { "—" } else { row.value.as_str() }).color(color)
                };
                if lit {
                    value = value.background_color(P.surface_hi);
                }
                if ui.add(egui::Label::new(value).sense(egui::Sense::click())).clicked() {
                    clicked = Some(n);
                }
                if row.kind == Field::Removed {
                    ui.label(RichText::new("taken off").color(P.faint).small());
                } else if let Some(was) = row.was.as_ref().filter(|w| *w != "new") {
                    ui.label(RichText::new(if was.is_empty() { "—" } else { was.as_str() }).color(P.faint).strikethrough());
                }
            });
            ui.end_row();
        }
    });
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credit(id: u128, name: &str, role: &str) -> CreditRow {
        CreditRow { artist_id: Some(Uuid::from_u128(id)), name: name.into(), role: role.into(), alias_id: None, alias: None }
    }

    fn album() -> AlbumData {
        AlbumData { title: "Songs".into(), kind: "album".into(), release_date: "2019".into(), credits: vec![credit(1, "Ann", "primary"), credit(2, "Bo", "featured")] }
    }

    #[test]
    fn staged_changes_show_what_they_make_and_what_it_was() {
        let staged = [
            Edit::Title("Other songs".into()),
            Edit::Credit(Credit { artist: Some(Uuid::from_u128(2)), name: "Bo".into(), role: Some("producer".into()), alias: None, at: Some(0) }),
            Edit::Uncredit(Uuid::from_u128(1), None),
        ];
        let rows = rows(&album(), staged.iter());
        assert_eq!((rows[0].value.as_str(), rows[0].was.as_deref()), ("Other songs", Some("Songs")));
        assert_eq!(rows[1].was, None, "the kind is as it was");
        // Bo in another role: a credit of its own, first; featured stays.
        assert_eq!((rows[3].kind, rows[3].value.as_str(), rows[3].was.as_deref()), (Field::Credit(0), "Bo · producer", Some("new")));
        assert_eq!((rows[4].value.as_str(), rows[4].was.as_deref()), ("Bo · featured", None));
        assert_eq!((rows[5].kind, rows[5].was.as_deref()), (Field::Removed, Some("Ann · primary")));
        assert_eq!(rows.len(), 6);
    }

    #[test]
    fn one_artist_in_two_roles() {
        let mut data = album();
        let bo = |role: Option<&str>, alias: Option<Option<(Uuid, String)>>| Credit { artist: Some(Uuid::from_u128(2)), name: "Bo".into(), role: role.map(str::to_string), alias, at: None };
        credit_into(&mut data.credits, bo(Some("producer"), None));
        credit_into(&mut data.credits, bo(Some("producer"), None));
        assert_eq!(data.credits.iter().map(|c| c.role.as_str()).collect::<Vec<_>>(), ["primary", "featured", "producer"], "in a role once");
        // A name for one role leaves the other as it is; taking one role off leaves the other.
        credit_into(&mut data.credits, bo(Some("producer"), Some(Some((Uuid::from_u128(9), "B".into())))));
        assert_eq!(data.credits.iter().map(|c| c.alias.as_deref()).collect::<Vec<_>>(), [None, None, Some("B")]);
        apply(&mut data, &Edit::Uncredit(Uuid::from_u128(2), Some("featured".into())));
        assert_eq!(data.credits.iter().map(|c| (c.name.as_str(), c.role.as_str())).collect::<Vec<_>>(), [("Ann", "primary"), ("Bo", "producer")]);
    }

    #[test]
    fn an_album_without_artists_has_a_row_to_add_one() {
        let mut data = album();
        data.credits.clear();
        assert_eq!(rows(&data, [].iter()).last().map(|r| r.kind), Some(Field::AddArtist));
    }

    #[test]
    fn a_new_artist_is_new_and_a_moved_one_says_so() {
        let staged = [
            Edit::Credit(Credit { artist: None, name: "Cy".into(), role: None, alias: None, at: None }),
            Edit::Credit(Credit { artist: Some(Uuid::from_u128(2)), name: "Bo".into(), role: None, alias: None, at: Some(0) }),
        ];
        let rows = rows(&album(), staged.iter());
        let credits: Vec<_> = rows.iter().filter(|r| matches!(r.kind, Field::Credit(_))).map(|r| (r.value.as_str(), r.was.as_deref())).collect();
        assert_eq!(credits, [("Bo · featured", Some("place 2")), ("Ann · primary", Some("place 1")), ("Cy · featured", Some("new"))]);
    }
}
