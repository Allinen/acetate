//! The commandline's language: what's typed as `key:value`, the same everywhere. Searches
//! and filters (`catalog::query`), the track editor (`ui::edit::editor`) and the artist
//! page each have their own keys (a `Keys` table) and work out what a value means
//! themselves; the syntax is this one.
//!
//! * `|` separates commands: `status:demo | date:2020`.
//! * A command is a key, its value, and that key's arguments: `artist:Ann role:featured
//!   as:Annie`. Another `name:` in a value is split off only when it's one of the key's
//!   arguments, so `title:Intro: Part 2` needs no quotes. A table can make every key an
//!   argument of every other (a search: `og:yes title:x`, no `|` needed).
//! * Words before any key are plain words (a search's, the editor's "find what to change").
//! * `"…"` quotes a value: `title:"x title:y"`, `title:"a | b"`. `\|` is a `|` in a value too.
//! * `-key:value` takes one off; `-key` alone clears a list key.
//! * `#…` after a key that takes it: its place or numbers (`#2`, `#2-5`, `#1+`), right after
//!   the colon too (`cover:#3`).
//! * Some keys have fields: after them, any `name:` that isn't one of their arguments is a
//!   field (a source's: `source:distrokid album_uuid:… index:1`).

/// A place's keys.
pub trait Keys: Copy + PartialEq + std::fmt::Debug + Sized {
    /// Whether `-key:…` is understood (a search has no "take off").
    const MINUS: bool = true;
    /// The key a typed name means, if any.
    fn key(name: &str) -> Option<Self>;
    /// Its own name, as written back.
    fn name(self) -> &'static str;
    /// Whether `-key` alone means "clear it".
    fn is_list(self) -> bool {
        false
    }
    /// Whether `#…` after its value is its own.
    fn takes_hash(self) -> bool {
        false
    }
    /// Whether `arg` may follow `main` in its command.
    fn takes(main: Self, arg: Self) -> bool;
    /// After `main`, the key its fields are (any `name:` that isn't one of its arguments).
    fn field(_main: Self) -> Option<Self> {
        None
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Part<K> {
    pub key: K,
    /// A field: its name, as typed.
    pub field: Option<String>,
    /// `-key…`: off, or (without a colon) clear.
    pub minus: bool,
    /// `key:` (vs `-key` alone).
    pub colon: bool,
    /// The value, trimmed, without its quotes, `\|` as `|`.
    pub value: String,
    pub quoted: bool,
    /// What followed the last `#`, if there was one.
    pub hash: Option<String>,
    /// Every `#…`, in order (a cover moved: which, and where to).
    pub hashes: Vec<String>,
    /// One of the arguments of the part before it (else: a command of its own).
    pub arg: bool,
    /// Where the value starts in the text (bytes): a choice from a list replaces the value
    /// from here to `end`.
    pub value_start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line<K> {
    /// Words before any key.
    pub words: Vec<String>,
    pub parts: Vec<Part<K>>,
}

impl<K: Keys> Part<K> {
    /// Its key's name (a field: the field's).
    pub fn name(&self) -> &str {
        self.field.as_deref().unwrap_or(self.key.name())
    }
}

impl<K> Default for Line<K> {
    fn default() -> Self {
        Line { words: Vec::new(), parts: Vec::new() }
    }
}

impl<K: Keys> Line<K> {
    /// The commands: each a key and its arguments.
    pub fn commands(&self) -> impl Iterator<Item = &[Part<K>]> {
        let mut rest = &self.parts[..];
        std::iter::from_fn(move || {
            let (_, after) = rest.split_first()?;
            let n = 1 + after.iter().take_while(|p| p.arg).count();
            let (command, left) = rest.split_at(n);
            rest = left;
            Some(command)
        })
    }

    /// What the last part goes with, when it's an argument: its command up to it.
    pub fn last_goes_with(&self) -> &[Part<K>] {
        match self.parts.last() {
            Some(last) if last.arg => {
                let before = &self.parts[..self.parts.len() - 1];
                let at = before.iter().rposition(|p| !p.arg).unwrap_or(0);
                &before[at..]
            }
            _ => &[],
        }
    }
}

/// A token: where it starts, its text, and whether it starts inside a quoted value.
struct Token<'a> {
    at: usize,
    text: &'a str,
    in_quote: bool,
}

/// The text in tokens, commands apart (`|` outside quotes and not escaped).
fn commands_of<'a>(text: &'a str) -> Vec<Vec<Token<'a>>> {
    let mut commands = vec![Vec::new()];
    let mut start: Option<(usize, bool)> = None;
    let mut in_quote = false;
    let mut chars = text.char_indices().peekable();
    let end_token = |commands: &mut Vec<Vec<Token<'a>>>, start: &mut Option<(usize, bool)>, at: usize| {
        if let Some((s, quoted)) = start.take() {
            commands.last_mut().unwrap().push(Token { at: s, text: &text[s..at], in_quote: quoted });
        }
    };
    while let Some((i, ch)) = chars.next() {
        if ch == '\\' && chars.peek().is_some_and(|(_, c)| *c == '|') {
            // `\|`: part of the token.
            start.get_or_insert((i, in_quote));
            chars.next();
            continue;
        }
        if ch == '"' {
            let token = start.map(|(s, _)| &text[s..i]).unwrap_or("");
            if in_quote {
                in_quote = false;
            } else if opens_quote(token) {
                in_quote = true;
            }
            start.get_or_insert((i, false));
            continue;
        }
        if !in_quote && ch == '|' {
            end_token(&mut commands, &mut start, i);
            commands.push(Vec::new());
            continue;
        }
        if ch.is_whitespace() {
            end_token(&mut commands, &mut start, i);
        } else {
            start.get_or_insert((i, in_quote));
        }
    }
    end_token(&mut commands, &mut start, text.len());
    commands
}

/// Whether a `"` after this much of a token opens a quoted value: right after `key:`.
fn opens_quote(token: &str) -> bool {
    let Some(name) = token.strip_suffix(':') else { return false };
    let name = name.strip_prefix('-').unwrap_or(name);
    !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')
}

pub fn parse<K: Keys>(text: &str) -> Line<K> {
    let mut line = Line::default();
    for command in commands_of(text) {
        let first = line.parts.len();
        // The command's key (its first part), unless that takes something off.
        let mut main: Option<(K, bool)> = None;
        for token in command {
            let end = token.at + token.text.len();
            if !token.in_quote {
                let keyed = key_token::<K>(token.at, token.text);
                let part = match (keyed, main) {
                    (Some(part), None) => Some(part),
                    (Some(part), Some((m, _))) if K::takes(m, part.key) => Some(Part { arg: true, ..part }),
                    // Not one of its arguments: a field, if it has fields, else part of the value.
                    (_, Some((m, false))) => K::field(m).and_then(|f| field_token(token.at, token.text, f)),
                    _ => None,
                };
                if let Some(part) = part {
                    if main.is_none() {
                        main = Some((part.key, part.minus));
                    }
                    line.parts.push(part);
                    continue;
                }
            }
            let Some(part) = line.parts[first..].last_mut() else {
                line.words.push(token.text.to_string());
                continue;
            };
            if !token.in_quote && part.key.takes_hash() && is_hash(token.text) {
                part.hash = Some(token.text[1..].to_string());
                part.hashes.push(token.text[1..].to_string());
            }
            part.end = end;
        }
    }
    for part in &mut line.parts {
        let raw = text[part.value_start..part.end].trim();
        // The `#…`s aren't part of the value.
        let raw = if part.hash.is_some() && !part.quoted { without_hashes(raw) } else { raw };
        let value = if part.quoted {
            let inner = raw.strip_prefix('"').unwrap_or(raw);
            let (inner, after) = inner.split_once('"').unwrap_or((inner, ""));
            let after = after.trim();
            if after.is_empty() { inner.to_string() } else { format!("{inner} {after}") }
        } else {
            raw.to_string()
        };
        part.value = value.replace("\\|", "|");
    }
    line
}

/// A token that starts a part: `key:…`, `-key:…`, or `-key` (a list key).
fn key_token<K: Keys>(at: usize, token: &str) -> Option<Part<K>> {
    let (minus, rest) = match token.strip_prefix('-') {
        Some(rest) if K::MINUS => (true, rest),
        Some(_) => return None,
        None => (false, token),
    };
    let (name, value, colon) = match rest.split_once(':') {
        Some((name, value)) => (name, value, true),
        None => (rest, "", false),
    };
    if name.is_empty() || !name.chars().all(|c| c.is_alphabetic()) {
        return None;
    }
    let k = K::key(name)?;
    if !colon && !(minus && k.is_list()) {
        return None;
    }
    let value_start = at + token.len() - value.len();
    // `cover:#3`: a `#` right after the colon.
    let hashes: Vec<String> = if k.takes_hash() && !value.is_empty() && is_hash(value) { vec![value[1..].to_string()] } else { Vec::new() };
    let hash = hashes.last().cloned();
    Some(Part { key: k, field: None, minus, colon, value: String::new(), quoted: value.starts_with('"'), hash, hashes, arg: false, value_start, end: at + token.len() })
}

/// `name:…` as a field (`album_uuid:…`, `index:1`); `https://…` isn't one.
fn field_token<K: Keys>(at: usize, token: &str, key: K) -> Option<Part<K>> {
    let (name, value) = token.split_once(':')?;
    let named = name.starts_with(|c: char| c.is_ascii_alphabetic()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !named || value.starts_with("//") {
        return None;
    }
    let value_start = at + name.len() + 1;
    Some(Part { key, field: Some(name.to_string()), minus: false, colon: true, value: String::new(), quoted: value.starts_with('"'), hash: None, hashes: Vec::new(), arg: true, value_start, end: at + token.len() })
}

/// A value without the `#…` words at its end.
fn without_hashes(mut raw: &str) -> &str {
    loop {
        let start = raw.rfind(char::is_whitespace).map_or(0, |i| i + 1);
        if start < raw.len() && is_hash(&raw[start..]) {
            raw = raw[..start].trim_end();
        } else {
            return raw;
        }
    }
}

/// `#`, `#2`, `#2-5`, `#1+`.
pub fn is_hash(token: &str) -> bool {
    let Some(rest) = token.strip_prefix('#') else { return false };
    let rest = rest.strip_suffix('+').unwrap_or(rest);
    rest.split('-').count() <= 2 && rest.split('-').all(|n| n.chars().all(|c| c.is_ascii_digit()))
}

/// Quotes a value where it needs it, to be typed back: one with a colon, a quote, a `|`,
/// or a word that's a key (`-word` of a list key, `#…`).
pub fn quoted<K: Keys>(value: &str) -> String {
    let needs = value.contains(':') || value.contains('"') || value.contains('|') || value.split_whitespace().any(|w| w.starts_with('#') || (w.starts_with('-') && K::key(&w[1..]).is_some()));
    if needs { format!("\"{}\"", value.replace('"', "'")) } else { value.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small table: `a:` takes `b:`, `s:` has fields, `n:` takes `#`, `l` is a list.
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum T {
        A,
        B,
        C,
        S,
        F,
        N,
        L,
    }

    impl Keys for T {
        fn key(name: &str) -> Option<T> {
            Some(match name {
                "a" => T::A,
                "b" => T::B,
                "c" => T::C,
                "s" => T::S,
                "n" => T::N,
                "l" => T::L,
                _ => return None,
            })
        }
        fn name(self) -> &'static str {
            "t"
        }
        fn is_list(self) -> bool {
            self == T::L
        }
        fn takes_hash(self) -> bool {
            self == T::N
        }
        fn takes(main: T, arg: T) -> bool {
            main == T::A && arg == T::B
        }
        fn field(main: T) -> Option<T> {
            (main == T::S).then_some(T::F)
        }
    }

    fn parts(text: &str) -> Vec<(T, bool, String, bool)> {
        parse::<T>(text).parts.into_iter().map(|p| (p.key, p.minus, p.value, p.arg)).collect()
    }

    #[test]
    fn a_command_is_its_key_and_its_arguments() {
        assert_eq!(parts("a:x y b:z"), [(T::A, false, "x y".into(), false), (T::B, false, "z".into(), true)]);
        // Not an argument: part of the value.
        assert_eq!(parts("a:x c:y"), [(T::A, false, "x c:y".into(), false)]);
        assert_eq!(parts("c:Intro: Part 2")[0].2, "Intro: Part 2");
        // `|` starts another.
        assert_eq!(parts("a:x | c:y"), [(T::A, false, "x".into(), false), (T::C, false, "y".into(), false)]);
        assert_eq!(parts("a:x|c:y").len(), 2);
        let line = parse::<T>("a:x b:y | c:z | b:w");
        let commands: Vec<usize> = line.commands().map(|c| c.len()).collect();
        assert_eq!(commands, [2, 1, 1], "b: after | is a command of its own");
        assert!(!line.parts[3].arg);
    }

    #[test]
    fn quotes_and_escapes_keep_what_would_split() {
        assert_eq!(parts("c:\"a | b\" | a:x"), [(T::C, false, "a | b".into(), false), (T::A, false, "x".into(), false)]);
        assert_eq!(parts("c:a \\| b | a:x")[0].2, "a | b");
        assert_eq!(parts("a:\"x b:y\"")[0].2, "x b:y");
        assert_eq!(parts("a:\"x b:y\" b:z").len(), 2);
        // Unclosed: the rest of the text.
        assert_eq!(parts("c:\"No | Way")[0].2, "No | Way");
        // A quote inside a value isn't one.
        assert_eq!(parts("c:12\" vinyl | a:x").len(), 2);
        assert_eq!(quoted::<T>("a | b"), "\"a | b\"");
        assert_eq!(quoted::<T>("No: Way"), "\"No: Way\"");
        assert_eq!(quoted::<T>("Plain song"), "Plain song");
        assert_eq!(quoted::<T>("x -l"), "\"x -l\"");
    }

    #[test]
    fn words_minus_hashes_and_fields() {
        let line = parse::<T>("find this | c:x");
        assert_eq!((line.words.clone(), line.parts.len()), (vec!["find".to_string(), "this".to_string()], 1));
        assert_eq!(parts("-l | -c:x"), [(T::L, true, "".into(), false), (T::C, true, "x".into(), false)]);
        assert_eq!(parse::<T>("-c").words, ["-c"], "-c alone: not a list");
        let n = &parse::<T>("n:Some LP #2-5").parts[0];
        assert_eq!((n.value.as_str(), n.hash.as_deref()), ("Some LP", Some("2-5")));
        assert_eq!(parse::<T>("c:Track #1").parts[0].value, "Track #1");
        let line = parse::<T>("s:distrokid album_uuid:x index:2 url:https://a.b/c a:no");
        let fields: Vec<(Option<&str>, &str)> = line.parts.iter().map(|p| (p.field.as_deref(), p.value.as_str())).collect();
        assert_eq!(fields, [(None, "distrokid"), (Some("album_uuid"), "x"), (Some("index"), "2"), (Some("url"), "https://a.b/c"), (Some("a"), "no")]);
        assert_eq!(parse::<T>("s:https://x.y/z").parts.len(), 1);
        assert_eq!(parse::<T>("-s:#1 index:1").parts.len(), 1, "taking one off has no fields");
    }

    #[test]
    fn where_a_choice_goes() {
        let text = "c:demo | a:an";
        let p = &parse::<T>(text).parts[1];
        assert_eq!(&text[p.value_start..p.end], "an");
        let line = parse::<T>("a:x b:y");
        assert_eq!(line.last_goes_with().len(), 1);
    }
}
