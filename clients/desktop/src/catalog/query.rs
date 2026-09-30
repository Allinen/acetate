//! What's typed in a search or filter: plain words (matched anywhere, as before) and
//! `key:value` parts, the same everywhere: `artist:qlvarn`, `album:…`, `title:…`,
//! `year:2019`, `genre:…`, `status:demo`, `format:flac`, `flag:cover`, `kind:ep`,
//! `disc:2`, `track:5` (its number), `group:` (a collective, era, project it's in),
//! `source:youtube` (where it came from: a platform it has a source on), `og:` (original
//! files: `og:` or `og:yes`; `og:no` the rest; other words look in the original filename),
//! `bitrate:<192` (kbps: `<`, `<=`, `>`, `>=`, `=`, or a number alone for about that),
//! `rate:44.1k` (the sample rate, compared the same way), `format:lossy` / `lossless`. A value
//! runs until the next key, so `artist:lil quill year:2017` needs no quotes. A key can be
//! shortened to any start of it (`art:`, `yr` isn't one: the aliases are listed below),
//! and a word with a colon that isn't a key is a plain word.
//!
//! What a key does depends on where it's typed: a track list filters by it, the palette
//! searches only artists for `artist:`, the bulk change sets it (see `edit::bulk`).

use super::model::Track;
use super::Catalog;

/// A key, as understood.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Artist,
    /// Credited as featured (the bulk change; elsewhere the same as `artist`).
    Featured,
    Album,
    Title,
    Year,
    Genre,
    Status,
    Format,
    Flag,
    /// An album's kind (album, ep, single…).
    Kind,
    Disc,
    /// Track numbers (the bulk change: from this number).
    Number,
    /// A grouping it's in (a collective, era, project…), by name.
    Group,
    /// A platform it has a source on (youtube, soundcloud…).
    Source,
    /// Its file is an original file (an og file): yes, no, or words in its name. Alone
    /// (`og:`) it means yes.
    Og,
    /// Its file's bitrate in kbps, compared (`<192`, `>=320`, `128`: about that).
    Bitrate,
    /// Its file's sample rate (`44.1k`, `48000`, `>48k`).
    SampleRate,
}

/// Every key's names: the first is its own, the rest are other names for it. A typed key
/// is any start of one of these (at least two letters), the first match winning.
const KEYS: &[(Key, &[&str])] = &[
    (Key::Artist, &["artist", "artists", "by"]),
    (Key::Featured, &["featuring", "feat", "ft"]),
    (Key::Album, &["album", "on"]),
    (Key::Title, &["title", "name"]),
    (Key::Year, &["year", "released", "date"]),
    (Key::Genre, &["genre", "genres"]),
    (Key::Status, &["status"]),
    (Key::Format, &["format", "codec"]),
    (Key::Flag, &["flag", "missing", "issue"]),
    (Key::Kind, &["kind", "type"]),
    (Key::Disc, &["disc", "disk", "cd"]),
    (Key::Number, &["track", "number", "numbers", "no", "renumber"]),
    (Key::Group, &["group", "grouping", "collective"]),
    (Key::Source, &["source", "sources", "platform", "from"]),
    (Key::Og, &["og", "original"]),
    (Key::Bitrate, &["bitrate", "kbps"]),
    (Key::SampleRate, &["rate", "samplerate", "hz"]),
];

impl Key {
    /// The key a typed name means, if any. Exact names first ("no" is `number`, not the
    /// start of something), then starts of names (two letters at least).
    pub fn parse(name: &str) -> Option<Key> {
        let name = name.to_lowercase();
        if let Some((key, _)) = KEYS.iter().find(|(_, names)| names.contains(&name.as_str())) {
            return Some(*key);
        }
        if name.chars().count() < 2 {
            return None;
        }
        KEYS.iter().find(|(_, names)| names.iter().any(|n| n.starts_with(&name))).map(|(key, _)| *key)
    }

    /// Its own name.
    pub fn name(self) -> &'static str {
        KEYS.iter().find(|(k, _)| *k == self).map(|(_, names)| names[0]).unwrap_or("")
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    /// Plain words, lowercase.
    pub words: Vec<String>,
    /// `key:value` parts, values as typed (trimmed).
    pub fields: Vec<(Key, String)>,
}

impl Query {
    pub fn parse(text: &str) -> Query {
        let mut query = Query::default();
        for token in text.split_whitespace() {
            let keyed = token.split_once(':').and_then(|(name, rest)| Some((Key::parse(name)?, rest)));
            match keyed {
                Some((key, rest)) => query.fields.push((key, rest.to_string())),
                // After a key: part of its value. Else: a plain word.
                None => match query.fields.last_mut() {
                    Some((_, value)) => {
                        if !value.is_empty() {
                            value.push(' ');
                        }
                        value.push_str(token);
                    }
                    None => query.words.push(token.to_lowercase()),
                },
            }
        }
        for (_, value) in &mut query.fields {
            *value = value.trim().to_string();
        }
        query
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.words.is_empty() && self.fields.iter().all(|(_, v)| v.is_empty())
    }

    /// The value of the first `key:` part, if there's one.
    pub fn get(&self, key: Key) -> Option<&str> {
        self.fields.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str())
    }

    /// Whether the track (its place in the catalog) matches: every word somewhere in its
    /// text (title, artists, album) or `extra` (a file's path, say), and every part.
    pub fn matches_track(&self, catalog: &Catalog, i: u32, extra: &str) -> bool {
        let text = catalog.track_text(i);
        let extra = extra.to_lowercase();
        if !self.words.iter().all(|w| text.contains(w.as_str()) || extra.contains(w.as_str())) {
            return false;
        }
        let t = &catalog.tracks[i as usize];
        self.fields.iter().filter(|(k, v)| !v.is_empty() || *k == Key::Og).all(|(key, value)| track_has(catalog, t, *key, &value.to_lowercase()))
    }
}

/// Whether the track has `value` (lowercase) for `key`.
fn track_has(catalog: &Catalog, t: &Track, key: Key, value: &str) -> bool {
    let has = |text: &str| text.to_lowercase().contains(value);
    match key {
        Key::Artist | Key::Featured => {
            has(&t.artist_display)
                || t.credits.iter().any(|c| c.2.as_deref().is_some_and(has) || catalog.artist(c.0).is_some_and(|a| has(&a.name) || a.aliases.iter().any(|x| has(x))))
        }
        Key::Album => t.albums.iter().any(|m| catalog.album(m.0).is_some_and(|a| has(&a.title))),
        Key::Title => has(&t.title),
        Key::Year => t.release_date.as_deref().is_some_and(|d| d.starts_with(value)) || t.year.is_some_and(|y| y.to_string() == value),
        Key::Genre => t.genres.iter().any(|g| has(g)),
        Key::Status => t.status.to_lowercase().starts_with(value),
        Key::Format => t.codec.as_deref().is_some_and(|codec| format_matches(codec, value)),
        // A flag's kind ("missing_cover"): typed with spaces or underscores alike.
        Key::Flag => {
            let value = value.replace(' ', "_");
            catalog.issues.get(&t.id).is_some_and(|flags| flags.iter().any(|(_, kind)| kind.contains(&value)))
        }
        Key::Kind => t.albums.iter().any(|m| catalog.album(m.0).is_some_and(|a| a.kind.starts_with(value))),
        Key::Disc => value.parse::<i16>().is_ok_and(|d| t.albums.iter().any(|m| m.1 == Some(d))),
        Key::Number => value.parse::<i16>().is_ok_and(|n| t.albums.iter().any(|m| m.2 == Some(n))),
        // Its own groupings, or its albums'.
        Key::Group => {
            let named = |id: &uuid::Uuid| catalog.grouping(*id).is_some_and(|g| has(&g.name));
            t.groupings.iter().any(named) || t.albums.iter().any(|m| catalog.album(m.0).is_some_and(|a| a.groupings.iter().any(named)))
        }
        Key::Source => t.platforms.iter().any(|p| p.starts_with(value)),
        Key::Og => og_matches(t.original_filename.as_deref(), value),
        Key::Bitrate => t.bitrate.is_some_and(|b| compare(b as f64, value, 1.0, true)),
        Key::SampleRate => t.sample_rate.is_some_and(|r| compare(r as f64, value, 1000.0, false)),
    }
}

/// Whether a number matches a comparison typed for it: `<N`, `<=N`, `>N`, `>=N`, `=N`, or
/// `N` alone (about N if `loose`: within 3%, so 192 finds a VBR file's 193; else exactly).
/// `N` may end in `k` (times 1000, unless `unit` is 1000 already: then as typed, and a
/// number under 1000 counts in thousands too, so `rate:44.1` is 44.1 kHz).
fn compare(have: f64, value: &str, unit: f64, loose: bool) -> bool {
    let value = value.replace(' ', "");
    let (op, number) = ["<=", ">=", "<", ">", "="].iter().find_map(|op| value.strip_prefix(op).map(|n| (*op, n))).unwrap_or(("", value.as_str()));
    let (digits, k) = match number.strip_suffix(['k', 'K']) {
        Some(digits) => (digits, true),
        None => (number, false),
    };
    let Ok(mut want) = digits.parse::<f64>() else { return false };
    if k || (unit >= 1000.0 && want < 1000.0) {
        want *= 1000.0;
    }
    match op {
        "<" => have < want,
        "<=" => have <= want,
        ">" => have > want,
        ">=" => have >= want,
        "=" => have == want,
        _ if loose => (have - want).abs() <= want * 0.03,
        _ => have == want,
    }
}

/// Whether a codec matches `format:value` (lowercase): in its name, and WAV files, which
/// are PCM ("pcm_s16le"…), for `wav`.
fn format_matches(codec: &str, value: &str) -> bool {
    let codec = codec.to_lowercase();
    let lossless = ["flac", "alac", "pcm", "wavpack", "ape", "tta", "mlp", "truehd"].iter().any(|l| codec.starts_with(l));
    match value {
        "lossless" => lossless,
        "lossy" => !lossless,
        _ => codec.contains(value) || (matches!(value, "wav" | "wave" | "pcm") && codec.starts_with("pcm")),
    }
}

/// Whether an original filename (None: not an og file) matches `og:value` (lowercase):
/// empty or yes-like is "an og file", no-like "not one", anything else is looked for in
/// the name.
pub fn og_matches(original: Option<&str>, value: &str) -> bool {
    match value {
        "" | "yes" | "y" | "true" | "1" => original.is_some(),
        "no" | "n" | "false" | "0" => original.is_none(),
        _ => original.is_some_and(|name| name.to_lowercase().contains(value)),
    }
}

/// Lowercase letters and digits only: "Status: demo" and "statusdemo" alike.
pub fn squashed(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Whether `typed` is in `text` letter by letter, in order (both squashed): "stdemo"
/// finds "Status: demo".
pub fn loosely_in(typed: &str, text: &str) -> bool {
    let mut text = squashed(text).chars().collect::<Vec<_>>().into_iter();
    squashed(typed).chars().all(|c| text.any(|t| t == c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_values_and_words() {
        let q = Query::parse("dark artist:lil quill year:2017 remix");
        assert_eq!(q.words, ["dark"]);
        assert_eq!(q.fields, [(Key::Artist, "lil quill".into()), (Key::Year, "2017 remix".into())]);
        // Shortened keys, other names, and a colon that isn't a key.
        assert_eq!(Query::parse("art:x").fields, [(Key::Artist, "x".into())]);
        assert_eq!(Query::parse("released:2019").fields, [(Key::Year, "2019".into())]);
        assert_eq!(Query::parse("feat:bo").fields, [(Key::Featured, "bo".into())]);
        assert_eq!(Query::parse("no:5").fields, [(Key::Number, "5".into())]);
        assert_eq!(Query::parse("group:tide crew").fields, [(Key::Group, "tide crew".into())]);
        assert_eq!(Query::parse("source:youtube").fields, [(Key::Source, "youtube".into())]);
        assert_eq!(Query::parse("og: artist:kai").fields, [(Key::Og, "".into()), (Key::Artist, "kai".into())]);
        assert_eq!(Query::parse("original:no").fields, [(Key::Og, "no".into())]);
        assert_eq!(Query::parse("track:5").fields, [(Key::Number, "5".into())], "track: is its number, everywhere");
        let q = Query::parse("re:zero");
        assert_eq!((q.words.len(), q.fields.len()), (0, 1), "re: is the start of released");
        let q = Query::parse("x:y 12:30");
        assert_eq!(q.words, ["x:y", "12:30"]);
        assert!(q.fields.is_empty());
        assert!(Query::parse("  ").is_empty());
        assert_eq!(Query::parse("status:").get(Key::Status), Some(""));
    }

    #[test]
    fn loose_matching_ignores_spaces_and_punctuation() {
        assert!(loosely_in("statusdemo", "Status: demo"));
        assert!(loosely_in("st demo", "Status: demo"));
        assert!(loosely_in("gnone", "Genres: none"));
        assert!(!loosely_in("demos", "Status: demo"));
    }

    #[test]
    fn og_is_yes_no_or_words_in_the_name() {
        let og = Some("glass_harbor_MASTER_v7.wav");
        assert!(og_matches(og, "") && og_matches(og, "yes") && !og_matches(og, "no"));
        assert!(og_matches(None, "no") && !og_matches(None, "") && !og_matches(None, "master"));
        assert!(og_matches(og, "master") && !og_matches(og, "demo"));
    }

    #[test]
    fn format_wav_finds_pcm() {
        let q = Query::parse("format:wav");
        let has = |codec: &str| format_matches(codec, &q.fields[0].1);
        assert!(has("pcm_s16le") && has("pcm_s24le") && !has("flac"));
        assert!(format_matches("flac", "fla"));
        assert!(format_matches("flac", "lossless") && format_matches("pcm_s24le", "lossless") && !format_matches("mp3", "lossless"));
        assert!(format_matches("mp3", "lossy") && format_matches("aac", "lossy") && !format_matches("alac", "lossy"));
    }

    #[test]
    fn bitrates_and_sample_rates_compare() {
        assert_eq!(Query::parse("bitrate:<192").fields, [(Key::Bitrate, "<192".into())]);
        assert_eq!(Query::parse("rate:44.1k").fields, [(Key::SampleRate, "44.1k".into())]);
        assert!(compare(128.0, "<192", 1.0, true) && !compare(256.0, "<192", 1.0, true));
        assert!(compare(193.0, "192", 1.0, true), "about: VBR");
        assert!(!compare(160.0, "192", 1.0, true));
        assert!(compare(320.0, ">=320", 1.0, true) && compare(320.0, "=320", 1.0, true));
        assert!(compare(44100.0, "44.1k", 1000.0, false) && compare(44100.0, "44.1", 1000.0, false) && compare(44100.0, "44100", 1000.0, false));
        assert!(compare(96000.0, ">48k", 1000.0, false) && !compare(48000.0, ">48k", 1000.0, false));
        assert!(!compare(128.0, "fast", 1.0, true));
    }
}
