//! Numbers as words, the same on every client: sizes in decimal units, thousands
//! separated, "1 track" and "3 tracks".

/// `1.5 GB`: decimal units (B, KB, MB, GB, TB), one decimal above bytes.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 { format!("{n} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

/// `1,234,567`.
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `1 track`, `3 tracks`, `1,200 tracks`: the word with an `s` unless there is one.
pub fn count(n: usize, word: &str) -> String {
    format!("{} {word}{}", thousands(n), if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_numbers_and_counts() {
        assert_eq!((bytes(0).as_str(), bytes(999).as_str(), bytes(1_500).as_str()), ("0 B", "999 B", "1.5 KB"));
        assert_eq!((bytes(2_345_000_000).as_str(), bytes(3_000_000_000_000).as_str()), ("2.3 GB", "3.0 TB"));
        assert_eq!((thousands(7).as_str(), thousands(1234567).as_str()), ("7", "1,234,567"));
        assert_eq!((count(1, "track").as_str(), count(0, "track").as_str(), count(1200, "file").as_str()), ("1 track", "0 tracks", "1,200 files"));
    }
}
