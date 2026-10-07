//! Synced lyrics (LRC): lines with timestamps, `[mm:ss.xx]text`. A line may have several
//! timestamps (a chorus sung twice); tags like `[ar:Artist]` or `[offset:+200]` are
//! metadata (the offset, in milliseconds, moves every line earlier or later).
//!
//! Our own addition: a line's language, `[lang:ja]` after its times (`[00:12.34][lang:ja]…`,
//! or `[lang:ja]…` on a line without one), for a song in more than one. A line without it
//! is in the entry's language. Other players show the tag as text; this app never does.
//!
//! And parts of the song: a line that's only a heading in brackets, `[Verse 1: Wrenfield]`,
//! `[Chorus]`, `[Intro: Artist 1 & Artist 2]` (Genius writes them so). It needs no time
//! (it may have one): it lasts until the next heading. After the colon, who performs it.
//! Anything in brackets alone on a line is one, except a time and LRC's own tags
//! (`[ar:…]`, `[offset:…]`…).

use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub at: Duration,
    pub text: String,
    /// Its own language, if it says one (`[lang:…]`).
    pub language: Option<String>,
    /// The part of the song it's in (an index into the sections), if a heading says.
    pub section: Option<usize>,
}

/// A part of the song, from its heading: `[Verse 1: Wrenfield]` → "Verse 1", ["Wrenfield"].
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub label: String,
    pub artists: Vec<String>,
}

/// LRC's own tags (metadata, not headings).
const METADATA: &[&str] = &["ar", "al", "ti", "au", "by", "offset", "length", "re", "ve", "tool", "la", "lang", "#"];

/// The heading a tag (without its brackets) is, if it is one.
pub fn section(tag: &str) -> Option<Section> {
    let tag = tag.trim();
    if timestamp(tag).is_some() {
        return None;
    }
    let (label, artists) = match tag.split_once(':') {
        Some((label, artists)) => {
            if METADATA.contains(&label.trim().to_ascii_lowercase().as_str()) {
                return None;
            }
            (label.trim(), artists)
        }
        None => (tag, ""),
    };
    if !label.chars().next().is_some_and(char::is_alphabetic) {
        return None;
    }
    let artists = artists
        .replace(" and ", ",")
        .replace('&', ",")
        .split(',')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_string)
        .collect();
    Some(Section { label: label.to_string(), artists })
}

/// The heading a line is (times before it allowed), if it's only that.
pub fn heading(line: &str) -> Option<Section> {
    let mut rest = line.trim();
    while let Some(end) = rest.strip_prefix('[').and_then(|r| r.find(']')) {
        let tag = &rest[1..=end];
        if timestamp(tag).is_none() {
            return (rest[end + 2..].trim().is_empty()).then(|| section(tag)).flatten();
        }
        rest = &rest[end + 2..];
    }
    None
}

/// What plain (not synced) lyrics show: text, and headings between it.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text(String),
    Heading(Section),
}

/// Plain lyrics as blocks: the lines between headings together (without `[lang:…]`).
pub fn blocks(body: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut text: Vec<&str> = Vec::new();
    let flush = |text: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        // Blank lines around a heading are its spacing, not the lyrics'.
        while text.last().is_some_and(|l| l.trim().is_empty()) {
            text.pop();
        }
        if text.iter().any(|l| !l.trim().is_empty()) {
            let start = text.iter().position(|l| !l.trim().is_empty()).unwrap_or(0);
            blocks.push(Block::Text(text[start..].join("\n")));
        }
        text.clear();
    };
    for line in body.lines() {
        match heading(line) {
            Some(section) => {
                flush(&mut text, &mut blocks);
                blocks.push(Block::Heading(section));
            }
            None => text.push(without_language(line)),
        }
    }
    flush(&mut text, &mut blocks);
    blocks
}

/// A line without its `[lang:…]` tag (at its start).
fn without_language(line: &str) -> &str {
    let trimmed = line.trim_start();
    match trimmed.strip_prefix("[lang:").and_then(|r| r.find(']').map(|end| &r[end + 1..])) {
        Some(rest) => rest,
        None => line,
    }
}

/// The timed lines, in time order. Empty if nothing has a timestamp.
#[cfg(any(test, feature = "test-support"))]
pub fn parse(body: &str) -> Vec<Line> {
    parse_with_sections(body).0
}

/// The timed lines, in time order, and the parts of the song their `section` points into.
pub fn parse_with_sections(body: &str) -> (Vec<Line>, Vec<Section>) {
    let mut offset_ms: i64 = 0;
    let mut lines = Vec::new();
    let mut sections = Vec::new();
    for raw in body.lines() {
        if let Some(section) = heading(raw) {
            sections.push(section);
            continue;
        }
        let current = sections.len().checked_sub(1);
        let mut rest = raw.trim();
        let mut times = Vec::new();
        let mut language = None;
        while let Some(tag_end) = rest.strip_prefix('[').and_then(|r| r.find(']')) {
            let tag = &rest[1..=tag_end];
            rest = &rest[tag_end + 2..];
            if let Some(at) = timestamp(tag) {
                times.push(at);
            } else if let Some(value) = tag.strip_prefix("offset:") {
                offset_ms = value.trim().parse().unwrap_or(0);
            } else if let Some(value) = tag.strip_prefix("lang:") {
                language = Some(value.trim().to_string()).filter(|l| !l.is_empty());
            }
        }
        for at in times {
            lines.push((at, rest.trim().to_string(), language.clone(), current));
        }
    }
    lines.sort_by_key(|(at, ..)| *at);
    let lines = lines
        .into_iter()
        // A positive offset makes the lyrics come sooner.
        .map(|(at, text, language, section)| Line { at: Duration::from_millis((at - offset_ms).max(0) as u64), text, language, section })
        .collect();
    (lines, sections)
}

/// Plain (not synced) lyrics as shown: without the lines' `[lang:…]` tags.
#[cfg(test)]
pub fn plain(body: &str) -> String {
    body.lines().map(without_language).collect::<Vec<_>>().join("\n")
}

/// `mm:ss`, `mm:ss.xx` or `mm:ss.xxx` (a tag, without its brackets) in milliseconds.
pub fn timestamp(tag: &str) -> Option<i64> {
    let (minutes, seconds) = tag.split_once(':')?;
    let minutes: i64 = minutes.trim().parse().ok()?;
    let (whole, fraction) = seconds.split_once(['.', ':']).unwrap_or((seconds, "0"));
    let whole: i64 = whole.trim().parse().ok()?;
    if !fraction.chars().all(|c| c.is_ascii_digit()) || fraction.is_empty() {
        return None;
    }
    // ".5" is 500 ms, ".05" 50 ms, ".005" 5 ms.
    let millis = format!("{fraction:0<3}")[..3].parse::<i64>().ok()?;
    Some(minutes * 60_000 + whole * 1000 + millis)
}

/// The line sung at `position` (the last one started), if any.
pub fn current(lines: &[Line], position: Duration) -> Option<usize> {
    lines.partition_point(|l| l.at <= position).checked_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_repeats_offsets_and_the_current_line() {
        let lines = parse("[ar:Someone]\n[00:01.50]one\n[00:03.00][00:10.00]chorus\n[00:05.123]two\nnot timed\n");
        let text: Vec<_> = lines.iter().map(|l| (l.at.as_millis(), l.text.as_str())).collect();
        assert_eq!(text, [(1500, "one"), (3000, "chorus"), (5123, "two"), (10000, "chorus")]);
        assert_eq!(current(&lines, Duration::from_millis(500)), None);
        assert_eq!(current(&lines, Duration::from_millis(3000)), Some(1));
        assert_eq!(current(&lines, Duration::from_secs(60)), Some(3));

        let shifted = parse("[offset:+500]\n[00:02.00]a");
        assert_eq!(shifted[0].at, Duration::from_millis(1500), "a positive offset: sooner");
        assert!(parse("just words\nno times").is_empty());
    }

    #[test]
    fn headings_name_parts_of_the_song_and_who_sings_them() {
        let body = "[ar:Someone]\n[Intro]\n[00:01.00]one\n\n[Verse 1: Wrenfield & Someone Else, A and B]\n[00:05.00]two\n[00:09.00][Chorus]\n[00:10.00]three";
        let (lines, sections) = parse_with_sections(body);
        let labels: Vec<_> = sections.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["Intro", "Verse 1", "Chorus"], "a time on a heading is allowed; [ar:…] isn't one");
        assert_eq!(sections[1].artists, ["Wrenfield", "Someone Else", "A", "B"]);
        let parts: Vec<_> = lines.iter().map(|l| (l.text.as_str(), l.section)).collect();
        assert_eq!(parts, [("one", Some(0)), ("two", Some(1)), ("three", Some(2))]);
        assert_eq!(heading("[laughs] ha"), None, "only a heading alone on its line");

        let plain = blocks("[Verse]\nline a\n[lang:ja]line b\n\n[Chorus: X]\nline c");
        assert_eq!(
            plain,
            [
                Block::Heading(Section { label: "Verse".into(), artists: vec![] }),
                Block::Text("line a\nline b".into()),
                Block::Heading(Section { label: "Chorus".into(), artists: vec!["X".into()] }),
                Block::Text("line c".into()),
            ]
        );
    }

    #[test]
    fn a_line_may_say_its_language() {
        let lines = parse("[00:01.00]hello\n[00:02.00][lang:ja]こんにちは");
        assert_eq!(lines[0].language, None);
        assert_eq!((lines[1].text.as_str(), lines[1].language.as_deref()), ("こんにちは", Some("ja")));
        assert_eq!(plain("hello\n[lang:ja]こんにちは"), "hello\nこんにちは");
    }
}
