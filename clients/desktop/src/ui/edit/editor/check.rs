//! The check under a staged `parse:`: every name found and what it credits, the
//! corrections made by hand, and every track it changes (before → after, its credits, what
//! looks wrong), the ones to look at first. One list, a row each, kept to what's on screen
//! (thousands of tracks).
//!
//! Keys (the editor handles them, `TrackEditor::check_key`): on a name, Enter says who it
//! is (the line finds the artist), `c` the next artist with that name, `s` splits it, `w`
//! keeps a name whole (the line has the text it was found in: leave exactly the name), `n`
//! says it isn't a name (its part stays in the title); on a track, `x` leaves it out (or
//! back in); on a correction, Delete takes it back.

use std::collections::HashMap;

use eframe::egui::{self, Align2, Pos2, Rect, Sense, Vec2};
use uuid::Uuid;

use super::engine::{Engine, PlanState};
use super::resolve::{Names, PinKind};
use super::title_parse::{Credited, TitlesParsed};
use crate::catalog::Catalog;
use crate::ui::theme::{font, medium, semibold, PALETTE as P};
use crate::ui::widgets;

/// A correction by hand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fix {
    NotName,
    Split,
    Whole,
}

/// A row of the check.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    Heading(String),
    /// A name found (its place in `TitlesParsed::names`).
    Name(usize),
    Fixed(Fix, String),
    /// An artist chosen for a name.
    Chosen(String, Uuid),
    /// A track, and its title before.
    Track(Uuid, String),
}

impl Entry {
    pub fn selectable(&self) -> bool {
        !matches!(self, Entry::Heading(_))
    }
}

const ROW: f32 = 40.0;

/// The rows: names (to choose first, then what looks wrong, then new ones), corrections,
/// tracks (refused first, then with something to look at, then the rest; left out last).
pub fn entries(p: &TitlesParsed, names: &Names, engine: &Engine) -> Vec<Entry> {
    let refused: HashMap<Uuid, bool> = engine.plans.iter().map(|plan| (plan.track, matches!(&plan.state, PlanState::Ready(r) if r.refused.is_some()))).collect();
    let mut out = Vec::new();

    let undecided: Vec<&str> = p.undecided().iter().map(|n| n.text.as_str()).collect();
    let mut order: Vec<usize> = (0..p.names.len()).collect();
    order.sort_by_key(|&i| {
        let n = &p.names[i];
        let rank = match &n.credited {
            _ if undecided.contains(&n.text.as_str()) => 0,
            _ if n.why.is_some() => 1,
            Credited::Undecided(_) => 1,
            Credited::New(_) => 2,
            Credited::Artist { .. } | Credited::Own => 3,
        };
        (rank, std::cmp::Reverse(n.tracks), n.text.to_lowercase())
    });
    let new = p.names.iter().filter(|n| matches!(n.credited, Credited::New(_))).count();
    let mut heading = format!("Names · {}", p.names.len());
    if new > 0 {
        heading.push_str(&format!(" · {new} new"));
    }
    if !undecided.is_empty() {
        heading.push_str(&format!(" · {} to choose", undecided.len()));
    }
    out.push(Entry::Heading(heading));
    out.extend(order.into_iter().map(Entry::Name));

    let chosen: Vec<Entry> = p.names.iter().filter_map(|n| names.pinned(PinKind::Artist, &n.text).map(|id| Entry::Chosen(n.text.clone(), id))).collect();
    if !p.fixes.is_empty() || !chosen.is_empty() {
        out.push(Entry::Heading("By hand · Delete takes one back".into()));
        out.extend(p.fixes.not_names.iter().map(|n| Entry::Fixed(Fix::NotName, n.clone())));
        out.extend(p.fixes.split.iter().map(|n| Entry::Fixed(Fix::Split, n.clone())));
        out.extend(p.fixes.whole.iter().map(|n| Entry::Fixed(Fix::Whole, n.clone())));
        out.extend(chosen);
    }

    let mut tracks: Vec<(u8, usize)> = Vec::new();
    let mut unchanged = 0;
    for (i, (track, title)) in p.titles.iter().enumerate() {
        let Some(o) = p.outcomes.get(title) else { continue };
        let undecided = o.credits.iter().any(|c| matches!(p.names[c.name].credited, Credited::Undecided(_)));
        let rank = if p.skip.contains(track) {
            3
        } else if refused.get(track) == Some(&true) || undecided || o.problem.is_some() {
            0
        } else if !o.warnings.is_empty() {
            1
        } else if o.changes(title) {
            2
        } else {
            unchanged += 1;
            continue;
        };
        tracks.push((rank, i));
    }
    tracks.sort_by_key(|t| t.0);
    let count = |r: u8| tracks.iter().filter(|t| t.0 == r).count();
    let mut heading = format!("Tracks · {} change", p.changing());
    for (n, what) in [(count(0), "can't be"), (count(1), "to look at"), (count(3), "left out"), (unchanged, "unchanged")] {
        if n > 0 {
            heading.push_str(&format!(" · {n} {what}"));
        }
    }
    out.push(Entry::Heading(heading));
    out.extend(tracks.into_iter().map(|(_, i)| Entry::Track(p.titles[i].0, p.titles[i].1.clone())));
    out
}

/// The first row the keys can be on, from `at` going `down` (or up).
pub fn selectable(entries: &[Entry], at: usize, down: bool) -> Option<usize> {
    if down {
        (at..entries.len()).find(|&i| entries[i].selectable())
    } else {
        (0..=at.min(entries.len().saturating_sub(1))).rev().find(|&i| entries[i].selectable())
    }
}

/// What the rows are drawn from.
pub struct View<'a> {
    pub parsed: &'a TitlesParsed,
    pub names: &'a Names,
    pub catalog: &'a Catalog,
    pub engine: &'a Engine,
}

/// The check: the keys' help, then the rows (`cursor` lit; `scroll`: brought into view).
/// Returns the row clicked.
pub fn draw(ui: &mut egui::Ui, view: &View, entries: &[Entry], cursor: Option<usize>, scroll: bool, id: egui::Id) -> Option<usize> {
    let help = "↓ from the line · on a name: Enter who it is, c the next with that name, s split, w keep whole, n not a name · on a track: x leave out · Delete takes back";
    ui.label(egui::RichText::new(help).color(P.faint).small());
    let plans: HashMap<Uuid, usize> = view.engine.plans.iter().enumerate().map(|(i, p)| (p.track, i)).collect();
    let offset_id = id.with("offset");
    let mut area = egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]);
    if let Some(row) = cursor.filter(|_| scroll) {
        let offset = ui.ctx().data(|d| d.get_temp::<f32>(offset_id)).unwrap_or(0.0);
        let (top, height) = (row as f32 * ROW, ui.available_height());
        if top < offset || top + ROW > offset + height {
            area = area.vertical_scroll_offset((top - height / 3.0).max(0.0));
        }
    }
    let mut clicked = None;
    let output = area.show_rows(ui, ROW, entries.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for n in range {
            let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
            let entry = &entries[n];
            if let Entry::Heading(text) = entry {
                ui.painter().text(rect.left_bottom() + Vec2::new(4.0, -8.0), Align2::LEFT_BOTTOM, text, font(13.5, semibold()), P.muted);
                continue;
            }
            let rect = rect.shrink2(Vec2::new(0.0, 1.0));
            if cursor == Some(n) {
                ui.painter().rect_filled(rect, 6, P.surface_hi);
            } else if response.hovered() {
                ui.painter().rect_filled(rect, 6, P.surface);
            }
            let (top, bottom, color) = row(view, entry, &plans);
            let width = rect.width() - 12.0;
            widgets::text_in(ui.painter(), Rect::from_min_size(Pos2::new(rect.left() + 6.0, rect.top() + 3.0), Vec2::new(width, 18.0)), &top, font(13.5, medium()), color);
            widgets::text_in(ui.painter(), Rect::from_min_size(Pos2::new(rect.left() + 6.0, rect.top() + 21.0), Vec2::new(width, 15.0)), &bottom, font(11.5, medium()), P.faint);
            if response.clicked() {
                clicked = Some(n);
            }
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(offset_id, output.state.offset.y));
    clicked
}

/// A row's two lines and the first one's color.
fn row(view: &View, entry: &Entry, plans: &HashMap<Uuid, usize>) -> (String, String, egui::Color32) {
    let p = view.parsed;
    let artist = |id: Uuid| view.catalog.artist_label(id).or_else(|| view.names.stand_ins().get(&id).map(|n| format!("{n} (new)"))).unwrap_or_else(|| "(an artist)".into());
    match entry {
        Entry::Heading(text) => (text.clone(), String::new(), P.muted),
        Entry::Name(i) => {
            let n = &p.names[*i];
            let chosen = view.names.pinned(PinKind::Artist, &n.text).is_some();
            let (what, color) = match &n.credited {
                Credited::Artist { id, alias: Some(_) } => (format!("{} (credited as this name)", artist(*id)), P.text),
                Credited::Artist { id, alias: None } => (artist(*id), P.text),
                Credited::New(_) => ("a new artist".to_string(), P.accent),
                Credited::Undecided(ids) => (format!("{} artists have this name: c chooses one, Enter another", ids.len()), P.danger),
                Credited::Own => ("each track's own primary artists".to_string(), P.text),
            };
            let mut bottom = vec![format!("{} track{}", n.tracks, if n.tracks == 1 { "" } else { "s" }), n.roles.join(", ")];
            if chosen {
                bottom.push("chosen".into());
            }
            if n.whole {
                bottom.push("kept whole".into());
            }
            if let Some(why) = n.why {
                bottom.push(format!("⚠ {why}"));
            }
            (format!("“{}”  →  {what}", n.text), bottom.join(" · "), color)
        }
        Entry::Fixed(fix, name) => {
            let what = match fix {
                Fix::NotName => "not a name",
                Fix::Split => "split",
                Fix::Whole => "kept whole",
            };
            (format!("{what}: “{name}”"), "by hand".into(), P.text)
        }
        Entry::Chosen(name, id) => (format!("“{name}” is {}", artist(*id)), "chosen".into(), P.text),
        Entry::Track(track, before) => {
            let Some(o) = p.outcomes.get(before) else { return (before.clone(), String::new(), P.faint) };
            let refused = plans.get(track).and_then(|&i| match &view.engine.plans[i].state {
                PlanState::Ready(r) => r.refused.clone(),
                _ => None,
            });
            let skipped = p.skip.contains(track);
            let top = if skipped { format!("{before}  (left out)") } else if o.title == *before { before.clone() } else { format!("{before}  →  {}", o.title) };
            let mut bottom: Vec<String> = Vec::new();
            let credits: Vec<String> = o.credits.iter().map(|c| format!("{} ({})", p.names[c.name].text, c.role)).collect();
            if !credits.is_empty() {
                bottom.push(format!("+ {}", credits.join(", ")));
            }
            // Credited in another role already: worth a look.
            if let Some(PlanState::Ready(r)) = plans.get(track).map(|&i| &view.engine.plans[i].state) {
                for c in &o.credits {
                    if let Credited::Artist { id, .. } = p.names[c.name].credited
                        && let Some(had) = r.base.meta.credits.iter().find(|x| x.artist_id == id && x.role != c.role)
                    {
                        bottom.push(format!("{} is {} already", p.names[c.name].text, had.role));
                    }
                }
            }
            bottom.extend(o.warnings.iter().map(|w| format!("⚠ {}", w.text())));
            if let Some(why) = refused.filter(|_| !skipped) {
                bottom.push(why);
            }
            let color = if skipped {
                P.faint
            } else if plans.get(track).is_some_and(|&i| matches!(&view.engine.plans[i].state, PlanState::Ready(r) if r.refused.is_some())) {
                P.danger
            } else if o.warnings.is_empty() {
                P.accent
            } else {
                P.text
            };
            (top, bottom.join(" · "), color)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::title_credits::{Fixes, NameTable, Setting};

    fn parsed(titles: &[&str], fixes: Fixes) -> TitlesParsed {
        let setting = Setting::parse("feat [prod]", &|t| ["primary", "featured", "producer"].iter().find(|r| r.starts_with(t)).map(|r| r.to_string())).unwrap();
        let titles = titles.iter().enumerate().map(|(i, t)| (Uuid::from_u128(100 + i as u128), t.to_string())).collect();
        // Ann is an artist, Bo two of them, anyone else new.
        let mut credited = |name: &str| {
            Some(match name {
                "Ann" => Credited::Artist { id: Uuid::from_u128(1), alias: None },
                "Bo" => Credited::Undecided(vec![Uuid::from_u128(2), Uuid::from_u128(3)]),
                _ => Credited::New(None),
            })
        };
        TitlesParsed::new(&setting, titles, fixes, &NameTable::default(), &mut credited).unwrap()
    }

    fn shown(entries: &[Entry], p: &TitlesParsed) -> Vec<String> {
        entries
            .iter()
            .map(|e| match e {
                Entry::Heading(h) => format!("# {h}"),
                Entry::Name(i) => p.names[*i].text.clone(),
                Entry::Fixed(_, n) => format!("fixed {n}"),
                Entry::Chosen(n, _) => format!("chosen {n}"),
                Entry::Track(_, t) => format!("track {t}"),
            })
            .collect()
    }

    #[test]
    fn what_needs_a_look_comes_first() {
        let p = parsed(&["Song (feat. Ann)", "Plain", "Other (feat. Cy)", "Third (feat. Bo)", "Odd (feat. Dee]"], Fixes::default());
        let e = entries(&p, &Names::default(), &Engine::default());
        assert_eq!(
            shown(&e, &p),
            [
                "# Names · 4 · 2 new · 1 to choose",
                "Bo",
                "Dee]",
                "Cy",
                "Ann",
                "# Tracks · 4 change · 1 can't be · 1 to look at · 1 unchanged",
                "track Third (feat. Bo)",
                "track Odd (feat. Dee]",
                "track Song (feat. Ann)",
                "track Other (feat. Cy)",
            ]
        );
        // Headings aren't for the keys.
        assert_eq!(selectable(&e, 0, true), Some(1));
        assert_eq!(selectable(&e, 5, false), Some(4));
    }

    #[test]
    fn corrections_are_listed_to_take_back() {
        let fixes = Fixes { not_names: vec!["bong".into()], ..Default::default() };
        let mut p = parsed(&["Cool song! [bong]", "Song (feat. Bo)"], fixes);
        p.skip.push(Uuid::from_u128(101));
        let mut names = Names::default();
        names.pin(PinKind::Artist, "Bo", Uuid::from_u128(3));
        let e = entries(&p, &names, &Engine::default());
        let shown = shown(&e, &p);
        assert!(shown.contains(&"fixed bong".to_string()) && shown.contains(&"chosen Bo".to_string()), "{shown:?}");
        assert_eq!(shown.last().map(String::as_str), Some("track Song (feat. Bo)"), "left out: last");
    }
}
