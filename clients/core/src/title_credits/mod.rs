//! Credits written into titles ("Song (feat. Ann)", "Song [prod. Bo]", "Song + Cy"), found
//! by the rules the user picks, never guessed. A title alone can't say whether
//! "Song [Bo]" is produced by Bo or just called that, so nothing here tries to: each rule
//! does one thing, the user chooses the rules that fit the tracks in front of them, and
//! every result is shown (with what looks wrong about it) before anything is applied.
//!
//! * `Setting`: the rules and the overrides, as typed after `parse:` (see `setting`).
//! * `parse`: one title → the title without its credits, the credits (name, role, which
//!   rule found it), and warnings.
//! * The rules run in a fixed order whatever order they're typed in: the keywords
//!   (`prod`, `feat`, `w/`, `with`, `w`, `x`, `word:`), then `self`, then `+`, then bare
//!   brackets, then `prefix`. So `[prod. Dee]` is always taken by `prod`, never by `[prod]` as a name
//!   "prod. Dee".
//! * Where a credit ends (`find`): a keyword right after an opening bracket runs to its
//!   closing bracket; elsewhere to the first of an opening bracket, another rule's keyword,
//!   ` - ` or ` | `, the closing bracket around it, the end.
//! * Names are split on the separators (`,` `&` `+` wherever they are, ` x ` ` and ` with
//!   spaces around, by default),
//!   except where pieces next to each other are exactly the name of an artist in the
//!   library ("keep whole", `NameTable`).
//! * Cutting a credit out takes the brackets around it if nothing else is left in them,
//!   and the spaces around it. Nothing else in the title is touched.

mod find;
mod setting;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use uuid::Uuid;

pub use setting::{Setting, RULES};
use setting::SELF_WORDS;

/// Every artist's name and aliases, ignoring case: for keeping names whole (and, in the
/// editor, for matching the names found to artists).
#[derive(Debug, Default)]
pub struct NameTable {
    by_name: HashMap<String, Vec<Uuid>>,
}

impl NameTable {
    /// From (artist, a name or alias of it).
    pub fn new<S: AsRef<str>>(names: impl IntoIterator<Item = (Uuid, S)>) -> Self {
        let mut by_name: HashMap<String, Vec<Uuid>> = HashMap::new();
        for (id, name) in names {
            let ids = by_name.entry(name.as_ref().trim().to_lowercase()).or_default();
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        NameTable { by_name }
    }

    /// The artists called `name` (its name or an alias), ignoring case.
    pub fn get(&self, name: &str) -> &[Uuid] {
        self.by_name.get(&name.trim().to_lowercase()).map(Vec::as_slice).unwrap_or(&[])
    }

    fn has(&self, name: &str) -> bool {
        !self.get(name).is_empty()
    }
}

/// Corrections by hand to what's found (the editor's check), by name, ignoring case.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fixes {
    /// Not names: the part a name like this was found in stays in the title, and nothing in
    /// it is credited.
    pub not_names: Vec<String>,
    /// Split on every separator (the default ones too), even an artist's name.
    pub split: Vec<String>,
    /// Kept whole, even if no artist has the name.
    pub whole: Vec<String>,
}

impl Fixes {
    fn has(list: &[String], name: &str) -> bool {
        let name = name.trim().to_lowercase();
        list.iter().any(|n| n.trim().to_lowercase() == name)
    }
}

/// A credit found in a title.
#[derive(Debug, Clone, PartialEq)]
pub struct Credit {
    /// As written.
    pub name: String,
    /// The text it was found in, before it was split into names ("Ann, Bo & Cy").
    pub part: String,
    pub role: String,
    /// The rule that found it, as typed (`feat`, `[producer]`).
    pub rule: String,
    /// Kept whole: separators in it, but it's an artist's name in the library.
    pub whole: bool,
    /// The track's own primary artists (`prod. himself`, `(self-prod.)`; the `self` rule).
    pub own: bool,
}

/// What looks wrong about a result.
#[derive(Debug, Clone, PartialEq)]
pub enum Warning {
    /// The new title still has a keyword the server flags ("feat.", "prod."…).
    KeywordLeft,
    /// A name found looks like more (or less) than one artist's name.
    Name { name: String, why: &'static str },
    /// The new title's brackets don't match.
    Brackets,
    /// Nothing would be left of the title.
    EmptyTitle,
    /// A credit in a bracket that never closes was read as far as it goes.
    Unclosed,
}

impl Warning {
    /// About the title it leaves (`title_warnings`), not about what was found.
    pub fn about_title(&self) -> bool {
        matches!(self, Warning::KeywordLeft | Warning::Brackets | Warning::EmptyTitle)
    }

    pub fn text(&self) -> String {
        match self {
            Warning::KeywordLeft => "still has “feat.”, “prod.” or the like in it (stays flagged)".into(),
            Warning::Name { name, why } => format!("“{name}”: {why}"),
            Warning::Brackets => "its brackets don't match".into(),
            Warning::EmptyTitle => "the title would be empty".into(),
            Warning::Unclosed => "a credit in a bracket that never closes was read as far as it goes".into(),
        }
    }
}

/// A title, parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub title: String,
    /// In the order found (rule by rule, each left to right).
    pub credits: Vec<Credit>,
    pub warnings: Vec<Warning>,
}

/// `title` parsed with `setting` and the corrections. The same title, setting, names and
/// corrections always give the same result.
pub fn parse(title: &str, setting: &Setting, names: &NameTable, fixes: &Fixes) -> Parsed {
    let keywords = setting.keywords();
    let own_rule = setting.rules.iter().any(|r| r.kind == setting::Kind::Own);
    let mut text = title.to_string();
    let mut credits: Vec<Credit> = Vec::new();
    let mut warnings = Vec::new();
    for rule in &setting.rules {
        // Each part's names (a self-production mark: the mark itself stands for them); a
        // part with a name that isn't one stays as it is.
        let found: Vec<(find::Found, Vec<(String, bool)>)> = find::find(&text, rule, setting, &keywords)
            .into_iter()
            .map(|f| {
                let named = if f.names.is_empty() { vec![(text[f.cut.clone()].to_string(), false)] } else { split(&text[f.names.clone()], setting, names, fixes) };
                (f, named)
            })
            .filter(|(_, named)| !named.iter().any(|(n, _)| Fixes::has(&fixes.not_names, n)))
            .collect();
        for (f, named) in &found {
            let part = &text[f.names.clone()];
            for (n, (name, whole)) in named.iter().cloned().enumerate() {
                let lower = name.to_lowercase();
                let own = own_rule && (f.names.is_empty() || SELF_WORDS.contains(&lower.as_str()));
                let role = rule.role(n);
                if !credits.iter().any(|c| c.role == role && c.name.to_lowercase() == lower) {
                    credits.push(Credit { name, part: part.to_string(), role: role.to_string(), rule: rule.token(), whole, own });
                }
            }
            if f.unclosed {
                push(&mut warnings, Warning::Unclosed);
            }
        }
        // From the right, so the places of the ones before stay as they were.
        for (f, _) in found.iter().rev() {
            text = cut_out(&text, f.cut.start, f.cut.end);
        }
    }
    if text != title {
        text = text.trim_end_matches(|c: char| c.is_whitespace() || "-–—|,&+".contains(c)).to_string();
    }

    for c in credits.iter().filter(|c| !c.whole && !c.own) {
        if let Some(why) = suspicious(&c.name, &keywords) {
            push(&mut warnings, Warning::Name { name: c.name.clone(), why });
        }
    }
    for w in title_warnings(title, &text) {
        push(&mut warnings, w);
    }
    Parsed { title: text, credits, warnings }
}

/// What looks wrong about a title `before` became `after` (a keyword left in it, brackets
/// that don't match, nothing left): for the title a whole edit leaves too, not only one
/// parse's.
pub fn title_warnings(before: &str, after: &str) -> Vec<Warning> {
    let mut out = Vec::new();
    if server_flags(after) {
        out.push(Warning::KeywordLeft);
    }
    if after != before {
        let (groups, strays) = find::groups(after);
        if strays > 0 || groups.iter().any(|g| !g.closed) {
            out.push(Warning::Brackets);
        }
    }
    if after.trim().is_empty() {
        out.push(Warning::EmptyTitle);
    }
    out
}

fn push(warnings: &mut Vec<Warning>, w: Warning) {
    if !warnings.contains(&w) {
        warnings.push(w);
    }
}

/// `text` without `a..b`: the brackets around it too if nothing else is left in them, and
/// the spaces around it (one is kept between words).
fn cut_out(text: &str, mut a: usize, mut b: usize) -> String {
    let (groups, _) = find::groups(text);
    while let Some(g) = find::enclosing(&groups, a, b) {
        if !text[g.inner..a].trim().is_empty() || !text[b..g.inner_end].trim().is_empty() {
            break;
        }
        (a, b) = (g.open, g.end);
    }
    let (left, right) = (&text[..a], &text[b..]);
    let (l, mut r) = (left.trim_end(), right.trim_start());
    // "Song | prod. Dee | Remix": one bar (or dash) left between the parts, not two.
    let divider = |s: &str| s.chars().next().filter(|c| "|-–—".contains(*c)).filter(|c| s[c.len_utf8()..].chars().next().is_none_or(char::is_whitespace));
    let ends_in_divider = l.chars().next_back().is_some_and(|c| "|-–—".contains(c)) && l[..l.len() - l.chars().next_back().map_or(0, char::len_utf8)].ends_with(char::is_whitespace);
    if ends_in_divider && let Some(c) = divider(r) {
        r = r[c.len_utf8()..].trim_start();
    }
    if l.is_empty() || r.is_empty() {
        return format!("{l}{r}");
    }
    let spaced = l.len() != left.len() || r.len() != right.len();
    let tight = l.chars().next_back().is_some_and(find::is_opener) || r.chars().next().is_some_and(find::is_closer);
    format!("{l}{}{r}", if spaced && !tight { " " } else { "" })
}

/// A credit's text split into names: at every separator, then pieces next to each other
/// joined back where they're exactly an artist's name or alias (longest first), or kept
/// whole by hand; names split by hand split again on every separator.
fn split(text: &str, setting: &Setting, names: &NameTable, fixes: &Fixes) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for (name, whole) in pieces(text, setting, names, fixes) {
        if !Fixes::has(&fixes.split, &name) {
            out.push((name, whole));
            continue;
        }
        out.extend(pieces(&name, &every_separator(setting), &NameTable::default(), &Fixes::default()));
    }
    out
}

/// `name` as splitting it by hand splits it (the check's `s`): on every separator, the
/// default ones too, never kept whole. One piece: nothing to split.
pub fn split_by_hand(name: &str, setting: &Setting) -> Vec<String> {
    pieces(name, &every_separator(setting), &NameTable::default(), &Fixes::default()).into_iter().map(|p| p.0).collect()
}

fn every_separator(setting: &Setting) -> Setting {
    let mut every = setting.clone();
    for s in setting::SPLIT {
        if !every.split.iter().any(|x| x == s) {
            every.split.push(s.to_string());
        }
    }
    every.whole = false;
    every
}

fn pieces(text: &str, setting: &Setting, names: &NameTable, fixes: &Fixes) -> Vec<(String, bool)> {
    let mut seps: Vec<(usize, usize)> = Vec::new();
    for (i, c) in text.char_indices() {
        for s in &setting.split {
            if !s.chars().any(char::is_alphanumeric) {
                // A sign (`,` `&` `+`): wherever it is, "Ann&Bo" too.
                if text[i..].starts_with(s.as_str()) {
                    seps.push((i, i + s.len()));
                }
            } else if c.is_whitespace() {
                // A word (`x`, `and`): with spaces on both sides ("Max" isn't split).
                let at = find::skip_space(text, i);
                if let Some(e) = find::match_ci(text, at, s)
                    && text[e..].starts_with(char::is_whitespace)
                {
                    seps.push((i, find::skip_space(text, e)));
                }
            }
        }
    }
    seps.sort();
    let mut pieces: Vec<(usize, usize)> = Vec::new();
    let mut from = 0;
    for (s, e) in seps {
        if s < from {
            from = from.max(e);
            continue;
        }
        pieces.push((from, s));
        from = e;
    }
    pieces.push((from, text.len()));
    let pieces: Vec<(usize, usize)> = pieces
        .into_iter()
        .filter_map(|(s, e)| {
            let piece = &text[s..e];
            let start = s + (piece.len() - piece.trim_start().len());
            let end = s + piece.trim_end().len();
            (start < end).then_some((start, end))
        })
        .collect();

    let mut out = Vec::new();
    let mut i = 0;
    'pieces: while i < pieces.len() {
        for j in (i + 1..pieces.len()).rev() {
            let joined = &text[pieces[i].0..pieces[j].1];
            let whole = (setting.whole && names.has(joined)) || Fixes::has(&fixes.whole, joined);
            if whole && !Fixes::has(&fixes.split, joined) {
                out.push((joined.to_string(), true));
                i = j + 1;
                continue 'pieces;
            }
        }
        out.push((text[pieces[i].0..pieces[i].1].to_string(), false));
        i += 1;
    }
    out
}

/// What looks wrong about a name found, if anything.
fn suspicious(name: &str, keywords: &[&str]) -> Option<&'static str> {
    let has_word = |words: &[&str]| name.char_indices().any(|(i, _)| find::keyword_at(name, i, words).is_some());
    if name.chars().any(|c| find::is_opener(c) || find::is_closer(c)) {
        Some("has a bracket in it")
    } else if has_word(SERVER_KEYWORDS) || has_word(keywords) {
        Some("has a keyword in it")
    } else if name.contains([',', '&', '+']) || has_word(&["x", "and"]) {
        Some("might be more than one artist")
    } else if SELF_WORDS.contains(&name.trim().to_lowercase().as_str()) {
        Some("means the track's own artist? (the self rule)")
    } else if name.chars().count() == 1 {
        Some("one character")
    } else if name.chars().count() > 40 {
        Some("very long for a name")
    } else {
        None
    }
}

/// The server's "credits in the title" check (`check_track`), the same regex by hand.
#[cfg(test)]
const SERVER_REGEX: &str = r"(^|[[:space:](\[{-])(feat\.?|ft\.?|featuring|w/(?!o)|prod\.?|produced by)([[:space:]]|$)";
const SERVER_KEYWORDS: &[&str] = &["featuring", "feat.", "feat", "ft.", "ft", "w/", "produced by", "prod.", "prod"];

/// Whether the server would flag `title` as having credits in it (`SERVER_REGEX`).
pub fn server_flags(title: &str) -> bool {
    title.char_indices().any(|(i, _)| {
        let before = title[..i].chars().next_back();
        if !before.is_none_or(|c| c.is_whitespace() || "([{-".contains(c)) {
            return false;
        }
        SERVER_KEYWORDS.iter().any(|k| {
            let rest = &title[i..];
            // `produced by` is one space there, not any.
            rest.get(..k.len()).is_some_and(|s| s.eq_ignore_ascii_case(k)) && rest[k.len()..].chars().next().is_none_or(char::is_whitespace)
        })
    })
}
