//! Synced lyrics (LRC): lines with timestamps, `[mm:ss.xx]text`. A line may have several
//! timestamps (a chorus sung twice); tags like `[ar:Artist]` or `[offset:+200]` are
//! metadata (the offset, in milliseconds, moves every line earlier or later).

use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub at: Duration,
    pub text: String,
}

/// The timed lines, in time order. Empty if nothing has a timestamp.
pub fn parse(body: &str) -> Vec<Line> {
    let mut offset_ms: i64 = 0;
    let mut lines = Vec::new();
    for raw in body.lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        while let Some(tag_end) = rest.strip_prefix('[').and_then(|r| r.find(']')) {
            let tag = &rest[1..=tag_end];
            rest = &rest[tag_end + 2..];
            if let Some(at) = timestamp(tag) {
                times.push(at);
            } else if let Some(value) = tag.strip_prefix("offset:") {
                offset_ms = value.trim().parse().unwrap_or(0);
            }
        }
        for at in times {
            lines.push((at, rest.trim().to_string()));
        }
    }
    lines.sort_by_key(|(at, _)| *at);
    lines
        .into_iter()
        // A positive offset makes the lyrics come sooner.
        .map(|(at, text)| Line { at: Duration::from_millis((at - offset_ms).max(0) as u64), text })
        .collect()
}

/// `mm:ss`, `mm:ss.xx` or `mm:ss.xxx` in milliseconds.
fn timestamp(tag: &str) -> Option<i64> {
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
}
