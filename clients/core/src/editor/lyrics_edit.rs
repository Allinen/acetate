//! Editing a track's lyrics, with no UI: the entries a track has (loaded and saved with the
//! revision they build on), and the text of an entry as lines to time and change (a draft:
//! tap along while it plays, nudge, rewrite, add and remove lines, take changes back).
//! The text stays exactly as it was written until a line is changed. What it writes is LRC
//! (see `crate::lyrics`).

use uuid::Uuid;

use crate::api::{Api, ApiError};
use crate::details::{Lyrics, TrackDetail};
use crate::lyrics;

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// Its (first) time as written (before the offset), in milliseconds.
    pub at: Option<i64>,
    /// More times, as written (a chorus sung again): kept as they are.
    pub more: Vec<String>,
    /// Its own language (`[lang:…]`), if it says one.
    pub language: Option<String>,
    /// The words.
    pub text: String,
    /// A tag line (`[ar:…]`, `[offset:…]`): kept, not timed.
    pub tag: bool,
    /// A heading (`[Verse 1: Wrenfield]`): `text` is what's in its brackets.
    pub heading: bool,
    /// The line as it was written, until it's changed.
    pub original: Option<String>,
}

impl Line {
    pub fn read(raw: &str) -> Line {
        let mut line = Line { at: None, more: Vec::new(), language: None, text: String::new(), tag: false, heading: false, original: Some(raw.to_string()) };
        let mut rest = raw.trim_start();
        while let Some(end) = rest.strip_prefix('[').and_then(|r| r.find(']')) {
            let tag = &rest[1..=end];
            if let Some(at) = lyrics::timestamp(tag) {
                match line.at {
                    None => line.at = Some(at),
                    Some(_) => line.more.push(rest[..end + 2].to_string()),
                }
            } else if let Some(language) = tag.strip_prefix("lang:") {
                line.language = Some(language.trim().to_string()).filter(|l| !l.is_empty());
            } else if rest[end + 2..].trim().is_empty() && line.language.is_none() && line.more.is_empty() && lyrics::section(tag).is_some() {
                // A heading, alone on its line (a time before it allowed).
                line.heading = true;
                line.text = tag.to_string();
                return line;
            } else {
                // Another tag: a whole line of it is metadata; else it's part of the words.
                line.tag = line.at.is_none() && rest[end + 2..].trim().is_empty() && tag.contains(':');
                break;
            }
            rest = &rest[end + 2..];
        }
        line.text = rest.to_string();
        line
    }

    pub fn write(&self) -> String {
        if let Some(original) = &self.original {
            return original.clone();
        }
        let mut out = self.at.map(stamp).unwrap_or_default();
        out.extend(self.more.iter().cloned());
        if self.heading {
            out.push_str(&format!("[{}]", self.text.trim()));
            return out;
        }
        if let Some(language) = &self.language {
            out.push_str(&format!("[lang:{language}]"));
        }
        out.push_str(&self.text);
        out
    }
}

/// `[mm:ss.xx]`, as LRC writes a time.
pub fn stamp(ms: i64) -> String {
    format!("[{}]", clock(ms))
}

/// `mm:ss.xx`.
pub fn clock(ms: i64) -> String {
    let ms = ms.max(0);
    format!("{:02}:{:02}.{:02}", ms / 60_000, ms / 1000 % 60, ms % 1000 / 10)
}

/// Changes `Draft::undo` can take back.
const UNDO_DEPTH: usize = 200;

/// A line as a screen shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct LineView {
    pub text: String,
    /// When it shows, in milliseconds (its time with the offset taken into account); None: no time.
    pub shows_ms: Option<i64>,
    /// A heading (`[Verse 1: Wrenfield]`): `text` is what's in its brackets.
    pub heading: bool,
    /// A tag line (`[ar:…]`, `[offset:…]`): kept, not timed.
    pub tag: bool,
    pub language: Option<String>,
}

/// The text of one entry as lines to time and change.
pub struct Draft {
    lines: Vec<Line>,
    /// A trailing newline, kept.
    newline_at_end: bool,
    /// `[offset:…]`, in milliseconds (a positive one shows lines sooner).
    offset_ms: i64,
    /// The line tapping times next.
    cursor: usize,
    /// The lines before each change, newest last.
    undo: Vec<Vec<Line>>,
    original: String,
}

impl Draft {
    pub fn new(body: &str) -> Draft {
        let lines: Vec<Line> = body.lines().map(Line::read).collect();
        let offset_ms = lines
            .iter()
            .filter(|l| l.tag)
            .find_map(|l| l.text.trim().strip_prefix("[offset:").and_then(|r| r.strip_suffix(']')).and_then(|v| v.trim().parse().ok()))
            .unwrap_or(0);
        let cursor = lines.iter().position(|l| !l.tag && !l.heading && l.at.is_none()).or_else(|| lines.iter().position(|l| !l.tag && !l.heading)).unwrap_or(0);
        Draft { lines, newline_at_end: body.ends_with('\n'), offset_ms, cursor, undo: Vec::new(), original: body.to_string() }
    }

    /// The lyrics as text again.
    pub fn text(&self) -> String {
        let mut text = self.lines.iter().map(Line::write).collect::<Vec<_>>().join("\n");
        if self.newline_at_end {
            text.push('\n');
        }
        text
    }

    pub fn changed(&self) -> bool {
        self.text() != self.original
    }

    pub fn offset_ms(&self) -> i64 {
        self.offset_ms
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn set_cursor(&mut self, n: usize) {
        if n < self.lines.len() {
            self.cursor = n;
        }
    }

    pub fn view(&self) -> Vec<LineView> {
        self.lines
            .iter()
            .map(|l| LineView { text: l.text.clone(), shows_ms: l.at.map(|at| (at - self.offset_ms).max(0)), heading: l.heading, tag: l.tag, language: l.language.clone() })
            .collect()
    }

    fn remember(&mut self) {
        if self.undo.len() == UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.undo.push(self.lines.clone());
    }

    /// Takes back the last change. False if there's none.
    pub fn undo(&mut self) -> bool {
        let Some(lines) = self.undo.pop() else { return false };
        self.lines = lines;
        self.cursor = self.cursor.min(self.lines.len().saturating_sub(1));
        true
    }

    /// The next line after `n` that tapping along times (not a heading or a tag).
    fn next_sung(&self, n: usize) -> Option<usize> {
        (n + 1..self.lines.len()).find(|&i| !self.lines[i].tag && !self.lines[i].heading)
    }

    /// The line at the cursor starts at this position (milliseconds into the song), and the
    /// cursor moves to the next line to sing. The line timed, if there was one.
    pub fn tap(&mut self, position_ms: i64) -> Option<usize> {
        let n = self.cursor;
        let line = self.lines.get(n).filter(|l| !l.tag && !l.heading)?;
        let _ = line;
        self.remember();
        let offset = self.offset_ms;
        let line = &mut self.lines[n];
        line.at = Some((position_ms + offset).max(0));
        line.original = None;
        if let Some(next) = self.next_sung(n) {
            self.cursor = next;
        }
        Some(n)
    }

    /// Moves a line's time by `by_ms` (a line without one gets `from_ms` first).
    pub fn nudge(&mut self, n: usize, by_ms: i64, from_ms: i64) {
        let Some(line) = self.lines.get(n).filter(|l| !l.tag && !l.heading) else { return };
        let at = line.at.unwrap_or(from_ms + self.offset_ms);
        self.set_time(n, Some(at + by_ms));
    }

    /// Takes a line's time away.
    pub fn clear_time(&mut self, n: usize) {
        if self.lines.get(n).is_some_and(|l| l.at.is_some()) {
            self.set_time(n, None);
        }
    }

    fn set_time(&mut self, n: usize, at: Option<i64>) {
        self.remember();
        if let Some(line) = self.lines.get_mut(n) {
            line.at = at.map(|at| at.max(0));
            line.original = None;
        }
    }

    /// Sets a line's words (a heading's label). True if they changed.
    pub fn set_text(&mut self, n: usize, text: &str) -> bool {
        if self.lines.get(n).is_none_or(|l| l.tag || l.text == text) {
            return false;
        }
        self.remember();
        let line = &mut self.lines[n];
        line.text = text.to_string();
        line.original = None;
        true
    }

    /// Sets the language of a line (empty: the entry's). True if it changed.
    pub fn set_language(&mut self, n: usize, language: &str) -> bool {
        let language = Some(language.trim().to_string()).filter(|l| !l.is_empty());
        if self.lines.get(n).is_none_or(|l| l.tag || l.heading || l.language == language) {
            return false;
        }
        self.remember();
        let line = &mut self.lines[n];
        line.language = language;
        line.original = None;
        true
    }

    /// Adds an empty line after line `n` (at the start, with None); it becomes the chosen one.
    pub fn insert_after(&mut self, n: Option<usize>) {
        self.remember();
        let at = n.map_or(0, |n| n + 1).min(self.lines.len());
        self.lines.insert(at, Line { at: None, more: Vec::new(), language: None, text: String::new(), tag: false, heading: false, original: None });
        self.cursor = at;
    }

    /// Adds an empty heading before line `n`; it becomes the chosen one.
    pub fn insert_heading_before(&mut self, n: usize) {
        self.remember();
        let at = n.min(self.lines.len());
        self.lines.insert(at, Line { at: None, more: Vec::new(), language: None, text: String::new(), tag: false, heading: true, original: None });
        self.cursor = at;
    }

    /// Removes a line.
    pub fn remove(&mut self, n: usize) {
        if n >= self.lines.len() {
            return;
        }
        self.remember();
        self.lines.remove(n);
        self.cursor = n.min(self.lines.len().saturating_sub(1));
    }
}

/// A track's lyrics entries, as the server has them now.
pub async fn entries(api: &Api, track: Uuid) -> Result<Vec<Lyrics>, ApiError> {
    Ok(api.send_json::<TrackDetail>(reqwest::Method::GET, &format!("/api/tracks/{track}"), None).await?.lyrics)
}

/// Writes an entry: a new one on the track (`edit` None), or a change of one (its id and the
/// revision it was read at: refused as `stale` if someone saved it since). The entry and its
/// new revision.
pub async fn save(api: &Api, track: Uuid, edit: Option<(Uuid, Uuid)>, body: &str, official: bool, language: &str, reason: &str) -> Result<(Uuid, Uuid), ApiError> {
    #[derive(serde::Deserialize)]
    struct Answer {
        /// Only when made.
        id: Option<Uuid>,
        revision_id: Uuid,
    }
    let language = Some(language.trim()).filter(|l| !l.is_empty());
    let mut json = serde_json::json!({ "body": body, "is_official": official, "language": language, "reason": reason.trim() });
    let (method, path) = match edit {
        None => (reqwest::Method::POST, format!("/api/tracks/{track}/lyrics")),
        Some((id, base)) => {
            json["base_revision_id"] = serde_json::json!(base);
            (reqwest::Method::PUT, format!("/api/lyrics/{id}"))
        }
    };
    let answer: Answer = api.send_json(method, &path, Some(json)).await?;
    let entry = match edit {
        Some((id, _)) => id,
        None => answer.id.ok_or_else(|| ApiError { status: None, code: "invalid".into(), message: "The server didn't say which entry it made.".into(), extra_id: None })?,
    };
    Ok((entry, answer.revision_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untouched_lines_come_back_as_they_were() {
        let body = "[ar:Ann]\n[00:01.5][00:30.00]odd  spacing\n\nplain line\n";
        let d = Draft::new(body);
        assert_eq!(d.text(), body);
        assert!(!d.changed());
    }

    #[test]
    fn tapping_along_times_the_lines_and_skips_headings() {
        let mut d = Draft::new("[offset:500]\n[Verse 1]\none\ntwo\n[Chorus]\nthree");
        assert_eq!(d.cursor(), 2, "the first line to sing");
        assert_eq!(d.tap(1000), Some(2));
        assert_eq!(d.tap(2000), Some(3));
        assert_eq!(d.cursor(), 5, "past the heading");
        // With the offset: written 0.5 s later, shown when heard.
        assert_eq!(d.text(), "[offset:500]\n[Verse 1]\n[00:01.50]one\n[00:02.50]two\n[Chorus]\nthree");
        assert_eq!(d.view()[2].shows_ms, Some(1000));
        assert!(d.undo());
        assert!(d.view()[3].shows_ms.is_none());
    }

    #[test]
    fn lines_are_nudged_edited_added_and_removed() {
        let mut d = Draft::new("[00:10.00]a\nb");
        d.nudge(0, 100, 0);
        d.nudge(1, -100, 4000);
        assert_eq!(d.view()[0].shows_ms, Some(10100));
        assert_eq!(d.view()[1].shows_ms, Some(3900));
        d.clear_time(1);
        assert!(d.set_text(1, "bee"));
        d.insert_after(Some(1));
        assert!(d.set_text(2, "c"));
        d.insert_heading_before(0);
        assert!(d.set_text(0, "Verse 1: Ann"));
        d.remove(1);
        assert_eq!(d.text(), "[Verse 1: Ann]\nbee\nc");
        assert!(d.set_language(1, "ja"));
        assert_eq!(d.text(), "[Verse 1: Ann]\n[lang:ja]bee\nc");
        assert!(d.changed());
    }
}
