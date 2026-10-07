//! What each rule finds in a title, and the pieces they share: brackets, keywords as
//! words, where a credit stops. Places are byte offsets into the title as it is when the
//! rule runs (the rules before it have cut theirs out).

use std::ops::Range;

use super::setting::{Kind, Rule, Setting, Shape};

/// Brackets, the full-width ones of CJK titles too.
const PAIRS: &[(char, char, Shape)] = &[
    ('(', ')', Shape::Round),
    ('（', '）', Shape::Round),
    ('[', ']', Shape::Square),
    ('［', '］', Shape::Square),
    ('【', '】', Shape::Square),
    ('{', '}', Shape::Curly),
    ('｛', '｝', Shape::Curly),
];

pub fn is_opener(c: char) -> bool {
    PAIRS.iter().any(|p| p.0 == c)
}

pub fn is_closer(c: char) -> bool {
    PAIRS.iter().any(|p| p.1 == c)
}

/// A bracket and what's in it.
#[derive(Debug, Clone)]
pub struct Group {
    pub open: usize,
    /// Where what's in it starts (after the opening bracket).
    pub inner: usize,
    /// Where what's in it ends: the closing bracket, or (`closed` false) where it was
    /// left unclosed (the end, or an outer bracket's closing).
    pub inner_end: usize,
    /// After the closing bracket.
    pub end: usize,
    pub closed: bool,
    pub shape: Shape,
}

/// The brackets in `text`, matched (a closing bracket closes the nearest open one of its
/// kind; ones opened after that stay unclosed), and how many closing brackets close none.
pub fn groups(text: &str) -> (Vec<Group>, usize) {
    let mut out: Vec<Group> = Vec::new();
    let mut open: Vec<(usize, char)> = Vec::new();
    let mut strays = 0;
    for (i, c) in text.char_indices() {
        if let Some(&(_, close, shape)) = PAIRS.iter().find(|p| p.0 == c) {
            let inner = i + c.len_utf8();
            out.push(Group { open: i, inner, inner_end: text.len(), end: text.len(), closed: false, shape });
            open.push((out.len() - 1, close));
        } else if is_closer(c) {
            match open.iter().rposition(|o| o.1 == c) {
                Some(depth) => {
                    for &(g, _) in &open[depth + 1..] {
                        (out[g].inner_end, out[g].end) = (i, i);
                    }
                    let g = &mut out[open[depth].0];
                    (g.inner_end, g.end, g.closed) = (i, i + c.len_utf8(), true);
                    open.truncate(depth);
                }
                None => strays += 1,
            }
        }
    }
    (out, strays)
}

/// The innermost bracket `a..b` is in.
pub fn enclosing(groups: &[Group], a: usize, b: usize) -> Option<&Group> {
    groups.iter().filter(|g| g.inner <= a && b <= g.inner_end).max_by_key(|g| g.open)
}

/// Where `word` ends if `text` has it at `at`, ignoring case; a space in `word` stands for
/// any run of whitespace.
pub fn match_ci(text: &str, at: usize, word: &str) -> Option<usize> {
    let mut rest = text[at..].char_indices().peekable();
    let mut end = at;
    for w in word.chars() {
        if w == ' ' {
            let mut any = false;
            while let Some(&(i, c)) = rest.peek()
                && c.is_whitespace()
            {
                rest.next();
                end = at + i + c.len_utf8();
                any = true;
            }
            if !any {
                return None;
            }
            continue;
        }
        let (i, c) = rest.next()?;
        if !c.to_lowercase().eq(w.to_lowercase()) {
            return None;
        }
        end = at + i + c.len_utf8();
    }
    Some(end)
}

pub fn skip_space(text: &str, at: usize) -> usize {
    at + (text[at..].len() - text[at..].trim_start().len())
}

/// Where one of `words` ends if it's at `at` as a word: after the start, whitespace, an
/// opening bracket or a dash, and before whitespace or the end (as the server's check).
pub fn keyword_at(text: &str, at: usize, words: &[&str]) -> Option<usize> {
    if !text[..at].chars().next_back().is_none_or(|c| c.is_whitespace() || is_opener(c) || c == '-') {
        return None;
    }
    words
        .iter()
        .filter_map(|w| match_ci(text, at, w))
        .filter(|&e| text[e..].chars().next().is_none_or(char::is_whitespace))
        .max()
}

/// A dash between parts (" - ", " – ", " — ") starting with the whitespace at `at`:
/// where it ends (after the whitespace following it).
pub fn dash_at(text: &str, at: usize) -> Option<usize> {
    divider_at(text, at, "-–—")
}

/// A dash or a bar between parts (" | ", "Song | prod. Ann | Remix"): where credits stop.
fn stop_at(text: &str, at: usize) -> Option<usize> {
    divider_at(text, at, "-–—|")
}

fn divider_at(text: &str, at: usize, signs: &str) -> Option<usize> {
    if !text[at..].starts_with(char::is_whitespace) {
        return None;
    }
    let d = skip_space(text, at);
    let c = text[d..].chars().next()?;
    let after = d + c.len_utf8();
    let spaced = text[after..].chars().next().is_none_or(char::is_whitespace);
    (signs.contains(c) && spaced).then(|| skip_space(text, after))
}

/// A credit found: what's cut from the title, and where its names are.
#[derive(Debug, Clone)]
pub struct Found {
    pub cut: Range<usize>,
    pub names: Range<usize>,
    /// In a bracket that never closes, read to the end.
    pub unclosed: bool,
}

/// What `rule` finds in `text`, left to right, not overlapping. `keywords`: every keyword
/// rule's words (where a credit stops).
pub fn find(text: &str, rule: &Rule, setting: &Setting, keywords: &[&str]) -> Vec<Found> {
    match &rule.kind {
        // After whitespace or an opening bracket ("C++" isn't one).
        Kind::Plus => marked(text, keywords, |i| {
            let after = text[..i].chars().next_back().is_none_or(|p| p.is_whitespace() || is_opener(p));
            let plus = after && text[i..].starts_with('+') && !text[i + 1..].starts_with('+');
            plus.then_some(i + 1)
        }),
        Kind::Bracket(shape) => bare(text, *shape, setting),
        Kind::Own => own(text, &rule.kind.keywords()),
        Kind::Prefix => prefix(text, false).into_iter().collect(),
        Kind::LoosePrefix => prefix(text, true).into_iter().collect(),
        kind => {
            let words = kind.keywords();
            marked(text, keywords, |i| keyword_at(text, i, &words))
        }
    }
}

/// Credits that start with a mark (a keyword, a `+`): `mark` says where the names start if
/// one is at a place. Not at the very start of the title: something has to come before it
/// ("With You" isn't a credit).
fn marked(text: &str, keywords: &[&str], mark: impl Fn(usize) -> Option<usize>) -> Vec<Found> {
    let (groups, _) = groups(text);
    let mut out = Vec::new();
    let mut from = 0;
    for (i, _) in text.char_indices() {
        if i < from || text[..i].trim().is_empty() {
            continue;
        }
        let Some(start) = mark(i) else { continue };
        let around = enclosing(&groups, i, i);
        let stop = around.map_or(text.len(), |g| g.inner_end);
        let end = names_end(text, start, stop, keywords);
        let names = &text[start..end];
        let (s, e) = (start + (names.len() - names.trim_start().len()), start + names.trim_end().len());
        if s >= e {
            continue;
        }
        out.push(Found { cut: i..e, names: s..e, unclosed: around.is_some_and(|g| !g.closed) });
        from = e;
    }
    out
}

/// `(self-prod.)`, `self-produced`: a mark with no names (found as a keyword is, not at the
/// very start).
fn own(text: &str, marks: &[&str]) -> Vec<Found> {
    let (groups, _) = groups(text);
    let mut out = Vec::new();
    let mut from = 0;
    for (i, _) in text.char_indices() {
        if i < from || text[..i].trim().is_empty() {
            continue;
        }
        // Nothing comes after a mark: it can end at a closing bracket too.
        let before = text[..i].chars().next_back().is_none_or(|c| c.is_whitespace() || is_opener(c) || c == '-');
        let end = marks.iter().filter_map(|m| match_ci(text, i, m)).filter(|&e| text[e..].chars().next().is_none_or(|c| c.is_whitespace() || is_closer(c))).max();
        if before && let Some(e) = end {
            let unclosed = enclosing(&groups, i, i).is_some_and(|g| !g.closed);
            out.push(Found { cut: i..e, names: e..e, unclosed });
            from = e;
        }
    }
    out
}

/// Where names starting at `from` stop: an opening bracket, a keyword, a dash or a bar, or
/// `stop`.
fn names_end(text: &str, from: usize, stop: usize, keywords: &[&str]) -> usize {
    for (i, c) in text[from..stop].char_indices() {
        let i = from + i;
        if is_opener(c) || (i > from && keyword_at(text, i, keywords).is_some()) || stop_at(text, i).is_some() {
            return i;
        }
    }
    stop
}

/// Brackets of `shape` with only names in them: closed, nothing bracketed inside, and none
/// of the words that are never names (`Setting::keep`), no version number.
fn bare(text: &str, shape: Shape, setting: &Setting) -> Vec<Found> {
    let (groups, _) = groups(text);
    groups
        .iter()
        .filter(|g| g.shape == shape && g.closed)
        .filter(|g| !groups.iter().any(|o| o.open > g.open && o.open < g.inner_end))
        .filter_map(|g| {
            let inner = &text[g.inner..g.inner_end];
            let (s, e) = (g.inner + (inner.len() - inner.trim_start().len()), g.inner + inner.trim_end().len());
            (s < e && !kept(&text[s..e], &setting.keep)).then_some(Found { cut: g.open..g.end, names: s..e, unclosed: false })
        })
        .collect()
}

/// Whether a bracket's contents has one of `keep` in it as words, or a version number
/// (`v2`, `2019`).
fn kept(contents: &str, keep: &[String]) -> bool {
    let lower = contents.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let numbered = |w: &&str| {
        let digits = w.strip_prefix('v').unwrap_or(w);
        !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
    };
    if words.iter().any(numbered) {
        return true;
    }
    keep.iter().any(|k| {
        let k: Vec<&str> = k.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
        !k.is_empty() && words.windows(k.len()).any(|w| w == k.as_slice())
    })
}

/// "Ann x Bo - Song": everything before the first dash that isn't in a bracket, if
/// something comes after it. `loose`: a dash with a space on one side only counts too
/// ("Ann- Song", "Ann -Song"); one with none never does ("Jay-Ann").
fn prefix(text: &str, loose: bool) -> Option<Found> {
    let (groups, _) = groups(text);
    let dash = |i: usize| -> Option<(usize, usize)> {
        if let Some(e) = dash_at(text, i) {
            return Some((i, e));
        }
        let c = text[i..].chars().next()?;
        let after = i + c.len_utf8();
        let spaced = text[..i].ends_with(char::is_whitespace) || text[after..].starts_with(char::is_whitespace);
        (loose && "-–—".contains(c) && spaced).then(|| (i, skip_space(text, after)))
    };
    let (at, end) = text.char_indices().filter(|&(i, _)| !groups.iter().any(|g| g.open < i && i < g.end)).find_map(|(i, _)| dash(i))?;
    let names = &text[..at];
    let (s, e) = (names.len() - names.trim_start().len(), names.trim_end().len());
    (s < e && !text[end..].trim().is_empty()).then_some(Found { cut: 0..end, names: s..e, unclosed: false })
}
