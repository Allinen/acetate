//! The check under the staged `parse:` chips (all of them): every name found and what it
//! credits, the corrections made by hand, and every track they change, as the whole edit
//! leaves it (its title now → after every chip, the credits it gains, what looks wrong),
//! the ones to look at first. One list, a row each, kept to what's on screen (thousands of
//! tracks).
//!
//! Keys (the editor handles them, `TrackEditor::check_key`): on a name, Enter says who it
//! is (the line finds the artist), `c` the next artist with that name, `s` splits it, `w`
//! keeps a name whole (the line has the text it was found in: leave exactly the name), `n`
//! says it isn't a name (its part stays in the title); on a track, `x` leaves it out (or
//! back in); on a correction, Delete takes it back.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, Align2, Pos2, Rect, Sense, Vec2};
use uuid::Uuid;

use super::engine::{Engine, PlanState};
use super::resolve::{Names, PinKind};
use super::title_parse::{Credited, TitlesParsed};
use crate::catalog::Catalog;
use crate::title_credits::Warning;
use crate::ui::theme::{font, medium, semibold, PALETTE as P};
use crate::ui::widgets;

/// A correction by hand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fix {
    NotName,
    Split,
    Whole,
}

/// A row of the check. `usize` first: which `parse:` chip (the nth staged).
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    Heading(String),
    /// A name found (its place in the chip's `TitlesParsed::names`).
    Name(usize, usize),
    Fixed(usize, Fix, String),
    /// An artist chosen for a name.
    Chosen(String, Uuid),
    /// A track, and its title as the first chip with it saw it (until it's planned).
    Track(Uuid, String),
}

impl Entry {
    pub fn selectable(&self) -> bool {
        !matches!(self, Entry::Heading(_))
    }
}

const ROW: f32 = 40.0;

/// What looks wrong about a track: about what each chip found in it (an odd name…), and
/// about the title the whole edit leaves it (`plan`: a keyword left in it… once a later
/// chip has fixed what an earlier one couldn't, it's gone).
fn track_warnings(parses: &[Arc<TitlesParsed>], track: Uuid, plan: Option<&PlanState>) -> Vec<Warning> {
    let mut out: Vec<Warning> = Vec::new();
    let mut last_title: Option<(&str, &str)> = None;
    for p in parses {
        if let Some((before, o)) = p.titles.iter().find(|(t, _)| *t == track).and_then(|(_, title)| Some((title.as_str(), p.outcomes.get(title)?))) {
            for w in o.warnings.iter().filter(|w| !w.about_title()) {
                if !out.contains(w) {
                    out.push(w.clone());
                }
            }
            last_title = Some((last_title.map_or(before, |l| l.0), o.title.as_str()));
        }
    }
    let after = match plan {
        Some(PlanState::Ready(r)) => crate::title_credits::title_warnings(&r.base.meta.title, &r.meta.title),
        _ => last_title.map(|(before, after)| crate::title_credits::title_warnings(before, after)).unwrap_or_default(),
    };
    out.extend(after);
    out
}

/// Whether every chip with `track` leaves it out.
pub fn left_out(parses: &[Arc<TitlesParsed>], track: Uuid) -> bool {
    let with: Vec<&Arc<TitlesParsed>> = parses.iter().filter(|p| p.titles.iter().any(|(t, _)| *t == track)).collect();
    !with.is_empty() && with.iter().all(|p| p.skip.contains(&track))
}

/// The rows: names (to choose first, then what looks wrong, then new ones), corrections,
/// tracks (can't be changed first, then with something to look at, then the rest; left
/// out last).
pub fn entries(parses: &[Arc<TitlesParsed>], names: &Names, engine: &Engine) -> Vec<Entry> {
    let plans: HashMap<Uuid, &PlanState> = engine.plans.iter().map(|plan| (plan.track, &plan.state)).collect();
    let mut out = Vec::new();

    let mut order: Vec<(u8, usize, String, usize, usize)> = Vec::new();
    let (mut new, mut undecided_count) = (0, 0);
    for (k, p) in parses.iter().enumerate() {
        let undecided: Vec<&str> = p.undecided().iter().map(|n| n.text.as_str()).collect();
        undecided_count += undecided.len();
        new += p.names.iter().filter(|n| matches!(n.credited, Credited::New(_))).count();
        for (i, n) in p.names.iter().enumerate() {
            let rank = match &n.credited {
                _ if undecided.contains(&n.text.as_str()) => 0,
                _ if n.why.is_some() => 1,
                Credited::Undecided(_) => 1,
                Credited::New(_) => 2,
                Credited::Artist { .. } | Credited::Own => 3,
            };
            order.push((rank, usize::MAX - n.tracks, n.text.to_lowercase(), k, i));
        }
    }
    order.sort();
    let mut heading = format!("Names · {}", order.len());
    if new > 0 {
        heading.push_str(&format!(" · {new} new"));
    }
    if undecided_count > 0 {
        heading.push_str(&format!(" · {undecided_count} to choose"));
    }
    out.push(Entry::Heading(heading));
    out.extend(order.into_iter().map(|(.., k, i)| Entry::Name(k, i)));

    let mut by_hand = Vec::new();
    for (k, p) in parses.iter().enumerate() {
        by_hand.extend(p.fixes.not_names.iter().map(|n| Entry::Fixed(k, Fix::NotName, n.clone())));
        by_hand.extend(p.fixes.split.iter().map(|n| Entry::Fixed(k, Fix::Split, n.clone())));
        by_hand.extend(p.fixes.whole.iter().map(|n| Entry::Fixed(k, Fix::Whole, n.clone())));
    }
    let mut chosen: Vec<String> = Vec::new();
    for n in parses.iter().flat_map(|p| p.names.iter()) {
        if let Some(id) = names.pinned(PinKind::Artist, &n.text)
            && !chosen.contains(&n.text.to_lowercase())
        {
            chosen.push(n.text.to_lowercase());
            by_hand.push(Entry::Chosen(n.text.clone(), id));
        }
    }
    if !by_hand.is_empty() {
        out.push(Entry::Heading("By hand · Delete takes one back".into()));
        out.extend(by_hand);
    }

    // Every track a chip has, once, in the order they come.
    let mut seen: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    let mut tracks: Vec<(u8, Uuid, String)> = Vec::new();
    let mut unchanged = 0;
    for p in parses {
        for (track, title) in &p.titles {
            if !seen.insert(*track) {
                continue;
            }
            let outcomes: Vec<(&TitlesParsed, &super::title_parse::Outcome)> =
                parses.iter().filter_map(|p| p.titles.iter().find(|(t, _)| t == track).and_then(|(_, title)| Some((&**p, p.outcomes.get(title)?)))).collect();
            let undecided = outcomes.iter().any(|(p, o)| !p.skip.contains(track) && o.credits.iter().any(|c| matches!(p.names[c.name].credited, Credited::Undecided(_))));
            let problem = outcomes.iter().any(|(p, o)| !p.skip.contains(track) && o.problem.is_some());
            let warnings = !track_warnings(parses, *track, plans.get(track).copied()).is_empty();
            let (refused, changes) = match plans.get(track) {
                Some(PlanState::Ready(r)) => (r.refused.is_some(), r.meta.title != r.base.meta.title || r.meta.credits != r.base.meta.credits),
                _ => (false, outcomes.iter().any(|(_, o)| o.changes(title))),
            };
            let rank = if left_out(parses, *track) {
                3
            } else if refused || undecided || problem {
                0
            } else if warnings {
                1
            } else if changes {
                2
            } else {
                unchanged += 1;
                continue;
            };
            tracks.push((rank, *track, title.clone()));
        }
    }
    tracks.sort_by_key(|t| t.0);
    let count = |r: u8| tracks.iter().filter(|t| t.0 == r).count();
    let mut heading = format!("Tracks · {} change", count(0) + count(1) + count(2));
    for (n, what) in [(count(0), "can't be"), (count(1), "to look at"), (count(3), "left out"), (unchanged, "unchanged")] {
        if n > 0 {
            heading.push_str(&format!(" · {n} {what}"));
        }
    }
    out.push(Entry::Heading(heading));
    out.extend(tracks.into_iter().map(|(_, t, title)| Entry::Track(t, title)));
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
    pub parses: &'a [Arc<TitlesParsed>],
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
    let stand_ins = view.names.stand_ins();
    let artist = |id: Uuid| view.catalog.artist_label(id).or_else(|| stand_ins.get(&id).map(|n| format!("{n} (new)"))).unwrap_or_else(|| "(an artist)".into());
    // Which chip, when there's more than one.
    let chip = |k: usize| if view.parses.len() > 1 { format!("parse {}", k + 1) } else { String::new() };
    match entry {
        Entry::Heading(text) => (text.clone(), String::new(), P.muted),
        Entry::Name(k, i) => {
            let n = &view.parses[*k].names[*i];
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
            bottom.push(chip(*k));
            bottom.retain(|b| !b.is_empty());
            (format!("“{}”  →  {what}", n.text), bottom.join(" · "), color)
        }
        Entry::Fixed(k, fix, name) => {
            let what = match fix {
                Fix::NotName => "not a name",
                Fix::Split => "split",
                Fix::Whole => "kept whole",
            };
            let bottom = match chip(*k) {
                c if c.is_empty() => "by hand".to_string(),
                c => format!("by hand · {c}"),
            };
            (format!("{what}: “{name}”"), bottom, P.text)
        }
        Entry::Chosen(name, id) => (format!("“{name}” is {}", artist(*id)), "chosen".into(), P.text),
        Entry::Track(track, title) => {
            let plan = plans.get(track).map(|&i| &view.engine.plans[i].state);
            let skipped = left_out(view.parses, *track);
            let mut bottom: Vec<String> = Vec::new();
            let (top, refused) = match plan {
                // As the whole edit leaves it: the title, and the credits it gains.
                Some(PlanState::Ready(r)) => {
                    let (before, after) = (&r.base.meta.title, &r.meta.title);
                    let top = if skipped { format!("{before}  (left out)") } else if before == after { before.clone() } else { format!("{before}  →  {after}") };
                    let gained: Vec<&super::model::Credit> = r.meta.credits.iter().filter(|c| !r.base.meta.credits.iter().any(|b| b.artist_id == c.artist_id && b.role == c.role)).collect();
                    if !gained.is_empty() {
                        bottom.push(format!("+ {}", gained.iter().map(|c| format!("{} ({})", artist(c.artist_id), c.role)).collect::<Vec<_>>().join(", ")));
                    }
                    // Credited in another role already: worth a look.
                    for c in &gained {
                        if let Some(had) = r.base.meta.credits.iter().find(|b| b.artist_id == c.artist_id && b.role != c.role) {
                            bottom.push(format!("{} is {} already", artist(c.artist_id), had.role));
                        }
                    }
                    (top, r.refused.clone())
                }
                _ => (title.clone(), None),
            };
            bottom.extend(track_warnings(view.parses, *track, plan).iter().map(|w| format!("⚠ {}", w.text())));
            let warned = bottom.iter().any(|b| b.starts_with('⚠'));
            let color = if skipped {
                P.faint
            } else if refused.is_some() {
                P.danger
            } else if warned {
                P.text
            } else {
                P.accent
            };
            if let Some(why) = refused.filter(|_| !skipped) {
                bottom.push(why);
            }
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
                Entry::Name(_, i) => p.names[*i].text.clone(),
                Entry::Fixed(_, _, n) => format!("fixed {n}"),
                Entry::Chosen(n, _) => format!("chosen {n}"),
                Entry::Track(_, t) => format!("track {t}"),
            })
            .collect()
    }

    #[test]
    fn what_needs_a_look_comes_first() {
        let p = parsed(&["Song (feat. Ann)", "Plain", "Other (feat. Cy)", "Third (feat. Bo)", "Odd (feat. Dee]"], Fixes::default());
        let e = entries(&[Arc::new(p.clone())], &Names::default(), &Engine::default());
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
    fn every_chip_is_in_it() {
        // Two chips on the same tracks: the second parses what the first left.
        let first = Arc::new(parsed(&["Song (feat. Ann) [Dee]", "Other"], Fixes::default()));
        let mut second = parsed(&["Song [Dee]", "Other"], Fixes::default());
        second.skip.push(Uuid::from_u128(101));
        let parses = [first.clone(), Arc::new(second)];
        let e = entries(&parses, &Names::default(), &Engine::default());
        let names: Vec<(usize, &str)> = e.iter().filter_map(|x| if let Entry::Name(k, i) = x { Some((*k, parses[*k].names[*i].text.as_str())) } else { None }).collect();
        assert!(names.contains(&(0, "Ann")) && names.contains(&(1, "Dee")), "{names:?}");
        // A track once, even if both chips have it.
        assert_eq!(e.iter().filter(|x| matches!(x, Entry::Track(t, _) if *t == Uuid::from_u128(100))).count(), 1);
        // Left out only when every chip with it leaves it out.
        assert!(!left_out(&parses, Uuid::from_u128(101)));
        assert!(left_out(&parses[1..], Uuid::from_u128(101)));
    }

    #[test]
    fn a_later_chip_fixes_what_an_earlier_one_left() {
        // `feat` leaves "w/" in the title (flagged), `w/` takes it out after.
        let setting = |text: &str| Setting::parse(text, &|t| ["primary", "featured", "producer"].iter().find(|r| r.starts_with(t)).map(|r| r.to_string())).unwrap();
        let titles = vec![(Uuid::from_u128(100), "Song w/ Ann".to_string())];
        let mut credited = |_: &str| Some(Credited::Artist { id: Uuid::from_u128(1), alias: None });
        let first = TitlesParsed::new(&setting("feat"), titles.clone(), Fixes::default(), &NameTable::default(), &mut credited).unwrap();
        assert_eq!(first.outcomes["Song w/ Ann"].warnings, [Warning::KeywordLeft]);
        let second = TitlesParsed::new(&setting("w/"), titles, Fixes::default(), &NameTable::default(), &mut credited).unwrap();
        let parses = [Arc::new(first.clone()), Arc::new(second)];
        assert!(track_warnings(&parses, Uuid::from_u128(100), None).is_empty(), "the title the edit leaves is clean");
        assert_eq!(track_warnings(&parses[..1], Uuid::from_u128(100), None), [Warning::KeywordLeft], "with the first alone, it isn't");
    }

    #[test]
    fn corrections_are_listed_to_take_back() {
        let fixes = Fixes { not_names: vec!["bong".into()], ..Default::default() };
        let mut p = parsed(&["Cool song! [bong]", "Song (feat. Bo)"], fixes);
        p.skip.push(Uuid::from_u128(101));
        let mut names = Names::default();
        names.pin(PinKind::Artist, "Bo", Uuid::from_u128(3));
        let e = entries(&[Arc::new(p.clone())], &names, &Engine::default());
        let shown = shown(&e, &p);
        assert!(shown.contains(&"fixed bong".to_string()) && shown.contains(&"chosen Bo".to_string()), "{shown:?}");
        assert_eq!(shown.last().map(String::as_str), Some("track Song (feat. Bo)"), "left out: last");
    }
}
