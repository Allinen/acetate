//! A commandline declared once: its commands, what each one's value is, and what may follow
//! it. The one declaration drives parsing (a tree of nodes), the list of choices (what can be
//! typed where the line ends), the checks (what's missing or wrong, said in the preview), and
//! the help. The syntax is the commandline's (`super`): `|` between commands, `"…"` and `\|`,
//! `-key:value` and `-key`.
//!
//! * A **command** is a key and its value (`artist:Ann`). What may follow it is declared
//!   (`Next`): arguments, in any order (`only:own as:Annie`), each of which may have its own;
//!   arguments that depend on the value (`Next::Branch`); or fields, any `name:` at all (a
//!   source's). A `name:` that isn't one of those where it's typed is part of the value, so
//!   `title:Intro: Part 2` needs no quotes.
//! * A value is of a **kind** (`Kind`): words, a number, a place (`n:2`, `n:2-5`, `n:1+`),
//!   or something the page looks up (an artist, one of its names, a track…). The page lists
//!   choices and resolves values per kind, not per key.
//! * Which arguments are **needed** is declared (`Need`): required, one of a group, or only
//!   with another. `problems` says what's missing.
//! * A **place** is `n:`: the value is what, `n:` is where (`cover:3 n:1`: the third cover
//!   made the first; `artist:Ann n:2`).
//!
//! What the list shows where the line ends (`rows`):
//!
//! | Where the line ends | The list shows |
//! |---|---|
//! | nothing typed, or right after `\|` | the commands (those not used yet first) |
//! | a word, before any key | the commands it starts |
//! | in a value, not finished | that kind's choices (and the keys the last word starts) |
//! | a value that is exactly a choice and a space after it, or none goes on from it | the arguments allowed there (the needed ones first) |
//!
//! Taking a row (`take`) puts a value in place of the one being typed and, on Tab, a space
//! after it; a key row writes `key:`. Taking never writes the next key by itself.

use std::collections::HashMap;

use uuid::Uuid;

/// A word a value can be, and what it means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Word {
    pub word: &'static str,
    pub help: &'static str,
}

/// What a value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// No value (`-key` alone).
    Nothing,
    Text,
    Number,
    /// `yes` or `no`.
    YesNo,
    /// Where: `2`, `2-5` (disc 2, track 5), `1+` (in order from 1).
    Place,
    /// One of these words.
    Words(&'static [Word]),
    /// One of these words, or else a value of the other kind (`to:same`, `to:<an artist>`).
    Or(&'static [Word], &'static Kind),
    Artist,
    /// One of an artist's names: the artist is the value of the first of these arguments
    /// that's there, else the command's own.
    AliasOf(&'static [&'static str]),
    Track,
    Album,
    Grouping,
    Path,
}

const YES_NO: &[Word] = &[Word { word: "yes", help: "" }, Word { word: "no", help: "" }];

impl Kind {
    /// The words it can be.
    pub fn words(self) -> &'static [Word] {
        match self {
            Kind::Words(w) | Kind::Or(w, _) => w,
            Kind::YesNo => YES_NO,
            _ => &[],
        }
    }

    /// What it is besides its words: the kind the page looks up.
    pub fn looked_up(self) -> Kind {
        match self {
            Kind::Or(_, other) => *other,
            other => other,
        }
    }

    /// The word a value is (any case).
    pub fn word(self, value: &str) -> Option<&'static str> {
        self.words().iter().find(|w| w.word.eq_ignore_ascii_case(value.trim())).map(|w| w.word)
    }

    /// What names of it are pinned as (a choice from the list kept by its id).
    pub fn pin_tag(self) -> &'static str {
        match self.looked_up() {
            Kind::Artist => "artist",
            Kind::AliasOf(_) => "alias",
            Kind::Track => "track",
            Kind::Album => "album",
            Kind::Grouping => "grouping",
            _ => "",
        }
    }

    /// Whether a value can be this, as far as it can be told without the library (a page
    /// looks up the rest).
    pub fn check(self, value: &str) -> Result<(), String> {
        let value = value.trim();
        match self {
            Kind::Nothing if value.is_empty() => Ok(()),
            Kind::Nothing => Err("nothing goes after it".into()),
            Kind::Number if value.parse::<i64>().is_ok() => Ok(()),
            Kind::Number => Err(format!("“{value}” isn't a number")),
            Kind::Place if place(value).is_some() => Ok(()),
            Kind::Place => Err(format!("“{value}” isn't a place (2, 2-5 or 1+)")),
            Kind::Words(words) | Kind::Or(words, Kind::Nothing) if self.word(value).is_none() => {
                Err(format!("“{value}” isn't one of: {}", words.iter().map(|w| w.word).collect::<Vec<_>>().join(", ")))
            }
            Kind::YesNo if self.word(value).is_none() => Err(format!("“{value}”: yes or no")),
            _ => Ok(()),
        }
    }

    /// For an `AliasOf`: the node whose value is the artist (the first argument named
    /// that's there, else the command).
    pub fn artist_node(self, command: &Node) -> Option<&Node> {
        let Kind::AliasOf(from) = self.looked_up() else { return None };
        Some(from.iter().find_map(|k| command.find(k)).unwrap_or(command))
    }
}

/// A place: a track's numbers, or which one in a list. `2` → track 2; `2-5` → disc 2,
/// track 5; `1+` → in order from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    pub disc: Option<i16>,
    pub n: i16,
    pub in_order: bool,
}

pub fn place(value: &str) -> Option<Place> {
    let value = value.trim();
    let (rest, in_order) = match value.strip_suffix('+') {
        Some(rest) => (rest, true),
        None => (value, false),
    };
    let number = |s: &str| (!s.is_empty() && s.chars().all(|c| c.is_ascii_digit())).then(|| s.parse::<i16>().ok()).flatten();
    match rest.split_once('-') {
        Some((disc, n)) => Some(Place { disc: Some(number(disc)?), n: number(n)?, in_order }),
        None => Some(Place { disc: None, n: number(rest)?, in_order }),
    }
}

/// `-key:value` and `-key`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Minus {
    No,
    /// `-key:value` takes one off.
    Off,
    /// That, and `-key` alone clears it.
    OffOrClear,
}

/// Whether an argument must be there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    Optional,
    Required,
    /// One of the arguments with this group's name must be there.
    OneOf(&'static str),
    /// Only with this argument.
    With(&'static str),
}

/// What may follow a command or an argument.
#[derive(Debug, Clone, Copy)]
pub enum Next {
    None,
    /// These, in any order.
    Args(&'static [Spec]),
    /// These, by what the value is (as typed).
    Branch(fn(&str) -> &'static [Spec]),
    /// These, and any other `name:` as a field (a source's: `album_uuid:…`).
    Fields(&'static [Spec]),
}

/// A command or an argument.
#[derive(Debug, Clone, Copy)]
pub struct Spec {
    pub key: &'static str,
    pub aliases: &'static [&'static str],
    pub value: Kind,
    pub minus: Minus,
    pub need: Need,
    pub next: Next,
    /// Goes with `-key…` too (an argument of a command that takes something off).
    pub with_minus: bool,
    pub help: &'static str,
}

impl Spec {
    pub const fn new(key: &'static str, value: Kind, help: &'static str) -> Spec {
        Spec { key, aliases: &[], value, minus: Minus::No, need: Need::Optional, next: Next::None, with_minus: false, help }
    }

    pub const fn aliases(self, aliases: &'static [&'static str]) -> Spec {
        Spec { aliases, ..self }
    }

    pub const fn minus(self, minus: Minus) -> Spec {
        Spec { minus, ..self }
    }

    pub const fn need(self, need: Need) -> Spec {
        Spec { need, ..self }
    }

    pub const fn next(self, next: Next) -> Spec {
        Spec { next, ..self }
    }

    pub const fn with_minus(self) -> Spec {
        Spec { with_minus: true, ..self }
    }

    /// Whether a typed name is it (its key or an alias, any case).
    pub fn is(&self, name: &str) -> bool {
        self.key.eq_ignore_ascii_case(name) || self.aliases.iter().any(|a| a.eq_ignore_ascii_case(name))
    }

    /// What may follow it, with this value (and `-` or not).
    pub fn allowed(&self, value: &str, minus: bool) -> Vec<&'static Spec> {
        let all: &'static [Spec] = match self.next {
            Next::None => &[],
            Next::Args(a) | Next::Fields(a) => a,
            Next::Branch(f) => f(value.trim()),
        };
        all.iter().filter(|s| !minus || s.with_minus).collect()
    }

    fn has_fields(&self, minus: bool) -> bool {
        matches!(self.next, Next::Fields(_)) && !minus
    }
}

/// A field: any `name:` after a command with fields. Its name is the node's `field`.
pub static FIELD: Spec = Spec::new("field", Kind::Text, "");

/// A commandline's commands.
#[derive(Debug, Clone, Copy)]
pub struct Schema {
    pub commands: &'static [Spec],
}

impl Schema {
    pub fn command(&self, name: &str) -> Option<&'static Spec> {
        self.commands.iter().find(|s| s.is(name))
    }

    /// Every spec in it, once (the branches' too, for the values `samples` gives).
    pub fn every(&self, samples: &dyn Fn(Kind) -> Vec<String>) -> Vec<&'static Spec> {
        let mut out: Vec<&'static Spec> = Vec::new();
        let mut todo: Vec<&'static Spec> = self.commands.iter().collect();
        while let Some(s) = todo.pop() {
            if out.iter().any(|o| std::ptr::eq(*o, s)) {
                continue;
            }
            out.push(s);
            match s.next {
                Next::None => {}
                Next::Args(a) | Next::Fields(a) => todo.extend(a.iter()),
                Next::Branch(f) => {
                    for v in samples(s.value) {
                        todo.extend(f(&v).iter());
                    }
                }
            }
        }
        out
    }

    /// A value as it is to be typed: quoted when it would be read otherwise (it has a
    /// colon, a quote, a `|`, or a word that's `-key` of this commandline).
    pub fn quoted(&self, value: &str) -> String {
        let keys = self.every(&|k| k.words().iter().map(|w| w.word.to_string()).collect());
        let needs = value.contains(':') || value.contains('"') || value.contains('|') || value.split_whitespace().any(|w| w.strip_prefix('-').is_some_and(|k| keys.iter().any(|s| s.is(k))));
        if needs { format!("\"{}\"", value.replace('"', "'")) } else { value.to_string() }
    }
}

/// A command or an argument, as typed.
#[derive(Debug, Clone)]
pub struct Node {
    pub spec: &'static Spec,
    /// A field: its name, as typed.
    pub field: Option<String>,
    /// `-key…`.
    pub minus: bool,
    /// `key:` (vs `-key` alone).
    pub colon: bool,
    /// The value: trimmed, without its quotes, `\|` as `|`.
    pub value: String,
    pub quoted: bool,
    /// Where its key starts, where its value starts, and where its own text ends (bytes): a
    /// choice replaces `value_start..end`.
    pub start: usize,
    pub value_start: usize,
    pub end: usize,
    pub args: Vec<Node>,
}

impl Node {
    /// Its key's name (a field: the field's).
    pub fn name(&self) -> &str {
        self.field.as_deref().unwrap_or(self.spec.key)
    }

    /// The argument with this key, if it's there (its own, not its arguments').
    pub fn arg(&self, key: &str) -> Option<&Node> {
        self.args.iter().find(|a| a.field.is_none() && a.spec.key == key)
    }

    /// The argument with this key anywhere under it.
    pub fn find(&self, key: &str) -> Option<&Node> {
        self.args.iter().find_map(|a| if a.field.is_none() && a.spec.key == key { Some(a) } else { a.find(key) })
    }

    /// Its fields: (name, value).
    pub fn fields(&self) -> impl Iterator<Item = (&str, &str)> {
        self.args.iter().filter_map(|a| Some((a.field.as_deref()?, a.value.as_str())))
    }

    /// What may follow it now.
    pub fn allowed(&self) -> Vec<&'static Spec> {
        if self.field.is_some() {
            return Vec::new();
        }
        self.spec.allowed(&self.value, self.minus)
    }

    /// The kind its value is (`-key` alone: nothing).
    pub fn kind(&self) -> Kind {
        if self.colon { self.spec.value } else { Kind::Nothing }
    }

    /// The last node typed under it (itself when it has no arguments), and the nodes on the
    /// way there, outermost first.
    fn last_chain(&self) -> Vec<&Node> {
        let mut chain = vec![self];
        while let Some(last) = chain.last().unwrap().args.last() {
            chain.push(last);
        }
        chain
    }
}

/// A line: its commands.
#[derive(Debug, Clone, Default)]
pub struct Line {
    /// Words not in any command (before a key).
    pub words: Vec<String>,
    pub commands: Vec<Node>,
    /// The last command (after a `|`) has no key yet.
    pub fresh: bool,
    /// A word being typed at the end, outside any command: where it starts.
    pub open_word: Option<usize>,
}

impl Line {
    /// The command being typed, unless a `|` has started another.
    pub fn current(&self) -> Option<&Node> {
        if self.fresh { None } else { self.commands.last() }
    }
}

/// `key:…`, `-key:…`, `-key`: its name, minus, colon, and where the value starts.
fn key_parts(at: usize, token: &str, minus_ok: bool) -> Option<(&str, bool, bool, usize)> {
    let (minus, rest) = match token.strip_prefix('-') {
        Some(rest) if minus_ok => (true, rest),
        Some(_) => return None,
        None => (false, token),
    };
    let (name, value, colon) = match rest.split_once(':') {
        Some((name, value)) => (name, value, true),
        None => (rest, "", false),
    };
    if name.is_empty() || !name.chars().all(|c| c.is_alphabetic() || c == '_') {
        return None;
    }
    Some((name, minus, colon, at + token.len() - value.len()))
}

fn node(spec: &'static Spec, minus: bool, colon: bool, at: usize, value_start: usize, end: usize, quoted: bool) -> Node {
    Node { spec, field: None, minus, colon, value: String::new(), quoted, start: at, value_start, end, args: Vec::new() }
}

/// The value as typed between `value_start` and `end`.
fn value_of(text: &str, node: &Node) -> String {
    let raw = text[node.value_start..node.end].trim();
    let value = if node.quoted {
        let inner = raw.strip_prefix('"').unwrap_or(raw);
        let (inner, after) = inner.split_once('"').unwrap_or((inner, ""));
        let after = after.trim();
        if after.is_empty() { inner.to_string() } else { format!("{inner} {after}") }
    } else {
        raw.to_string()
    };
    value.replace("\\|", "|")
}

/// The node at `path` under `root`.
fn at_path<'a>(root: &'a mut Node, path: &[usize]) -> &'a mut Node {
    path.iter().fold(root, |n, &i| &mut n.args[i])
}

pub fn parse(schema: &Schema, text: &str) -> Line {
    let mut line = Line::default();
    let tokens = super::commands_of(text);
    let count = tokens.len();
    for (c, command) in tokens.into_iter().enumerate() {
        let mut root: Option<Node> = None;
        // The open nodes: indices from the root down to the one being typed.
        let mut path: Vec<usize> = Vec::new();
        let mut last_word = None;
        for token in command {
            let end = token.at + token.text.len();
            last_word = None;
            if !token.in_quote {
                if let Some(root) = root.as_mut() {
                    if let Some((at, n)) = argument(text, root, &path, token.at, token.text) {
                        path.truncate(at);
                        let parent = at_path(root, &path);
                        parent.args.push(n);
                        path.push(parent.args.len() - 1);
                        continue;
                    }
                } else if let Some((name, minus, colon, value_start)) = key_parts(token.at, token.text, true)
                    && let Some(spec) = schema.command(name)
                    && (colon && (!minus || spec.minus != Minus::No) || (minus && spec.minus == Minus::OffOrClear))
                {
                    let quoted = text[value_start..end].starts_with('"');
                    root = Some(node(spec, minus, colon, token.at, value_start, end, quoted));
                    continue;
                }
            }
            match root.as_mut() {
                Some(root) => at_path(root, &path).end = end,
                None => {
                    line.words.push(token.text.to_string());
                    last_word = Some(token.at);
                }
            }
        }
        if c + 1 == count {
            line.fresh = root.is_none();
            line.open_word = last_word.filter(|_| !text.ends_with(char::is_whitespace));
        }
        if let Some(mut root) = root {
            fill_values(text, &mut root);
            line.commands.push(root);
        }
    }
    line
}

/// A token as an argument of one of the open nodes (the innermost that takes it): the
/// depth it goes under, and the node.
fn argument(text: &str, root: &Node, path: &[usize], at: usize, token: &str) -> Option<(usize, Node)> {
    let mut chain = vec![root];
    for &i in path {
        chain.push(&chain.last().unwrap().args[i]);
    }
    let end = at + token.len();
    let (name, minus, colon, value_start) = key_parts(at, token, false).filter(|(_, _, colon, _)| *colon)?;
    let quoted = text[value_start..end].starts_with('"');
    for depth in (0..chain.len()).rev() {
        let open = chain[depth];
        if open.field.is_some() {
            continue;
        }
        let value = value_of(text, open);
        if let Some(spec) = open.spec.allowed(&value, open.minus).into_iter().find(|s| s.is(name)) {
            return Some((depth, node(spec, minus, colon, at, value_start, end, quoted)));
        }
        if open.spec.has_fields(open.minus) && !text[value_start..end].starts_with("//") && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            let mut n = node(&FIELD, false, true, at, value_start, end, quoted);
            n.field = Some(name.to_string());
            return Some((depth, n));
        }
    }
    None
}

fn fill_values(text: &str, node: &mut Node) {
    node.value = value_of(text, node);
    for arg in &mut node.args {
        fill_values(text, arg);
    }
}

/// What can go where the line ends.
pub enum At<'l> {
    /// A command is next: nothing typed, or a `|` just typed. `word`: one being typed (where
    /// it starts and what it is so far), to finish as a command's key.
    Command { word: Option<(usize, &'l str)> },
    /// In a value: the command, and the node on the way down to the one being typed (the
    /// last is that one). `ended`: a space after it.
    Value { chain: Vec<&'l Node>, ended: bool },
}

pub fn at<'l>(line: &'l Line, text: &'l str) -> At<'l> {
    match line.current() {
        None => At::Command { word: line.open_word.map(|w| (w, &text[w..])) },
        Some(command) => At::Value { chain: command.last_chain(), ended: text.ends_with(char::is_whitespace) },
    }
}

/// A row of the list under the line.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub label: String,
    pub sub: String,
    pub take: Take,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Take {
    /// `key:`, to type its value next; in place of the word typed from `at`, if any.
    Key { key: &'static str, at: Option<usize> },
    /// The value being typed becomes this (quoted where it needs it).
    Value(String),
    /// One of several with the same name: the value becomes the name, and that one is
    /// meant by it from now on.
    Pin(String, Uuid),
}

impl Row {
    pub fn value(label: impl Into<String>, sub: impl Into<String>) -> Row {
        let label = label.into();
        Row { take: Take::Value(label.clone()), label, sub: sub.into() }
    }

    pub fn pin(label: impl Into<String>, sub: impl Into<String>, id: Uuid) -> Row {
        let label = label.into();
        Row { take: Take::Pin(label.clone(), id), label, sub: sub.into() }
    }

    pub fn is_key(&self) -> bool {
        matches!(self.take, Take::Key { .. })
    }
}

/// The choices a page has for the kinds it looks up.
pub trait Lists {
    /// The choices for `kind` starting like `typed`, for `node` in `command`.
    fn choices(&self, kind: Kind, typed: &str, node: &Node, command: &Node) -> Vec<Row>;
}

fn key_row(spec: &'static Spec, at: Option<usize>) -> Row {
    Row { label: format!("{}:", spec.key), sub: spec.help.to_string(), take: Take::Key { key: spec.key, at } }
}

/// The arguments that can follow, innermost first, those not there yet (needed ones
/// first).
fn args_left(chain: &[&Node]) -> Vec<&'static Spec> {
    let mut out: Vec<&'static Spec> = Vec::new();
    for open in chain.iter().rev() {
        for spec in open.allowed() {
            if open.arg(spec.key).is_none() && !out.iter().any(|o| std::ptr::eq(*o, spec)) {
                out.push(spec);
            }
        }
    }
    out.sort_by_key(|s| matches!(s.need, Need::Optional | Need::With(_)));
    out
}

/// The list for where the line ends.
pub fn rows(schema: &Schema, line: &Line, text: &str, lists: &dyn Lists) -> Vec<Row> {
    match at(line, text) {
        At::Command { word } => {
            let typed = word.map(|(_, w)| w.to_lowercase()).unwrap_or_default();
            let mut specs: Vec<&'static Spec> = schema.commands.iter().filter(|s| std::iter::once(&s.key).chain(s.aliases).any(|k| k.starts_with(&typed))).collect();
            specs.sort_by_key(|s| line.commands.iter().any(|c| std::ptr::eq(c.spec, *s)));
            specs.into_iter().map(|s| key_row(s, word.map(|(at, _)| at))).collect()
        }
        At::Value { chain, ended } => {
            let node = *chain.last().unwrap();
            let kind = node.kind();
            let typed = node.value.trim();
            let mut choices: Vec<Row> = Vec::new();
            if kind != Kind::Nothing && node.field.is_none() {
                let lower = typed.to_lowercase();
                choices.extend(kind.words().iter().filter(|w| w.word.starts_with(&lower)).map(|w| Row::value(w.word, w.help)));
                choices.extend(lists.choices(kind.looked_up(), typed, node, chain[0]));
            }
            let done = kind == Kind::Nothing || (ended && !typed.is_empty() && (choices.is_empty() || choices.iter().any(|r| r.label.eq_ignore_ascii_case(typed))));
            if done {
                return args_left(&chain).into_iter().map(|s| key_row(s, None)).collect();
            }
            // The last word of a value can be a key being typed: `artist:Ann on` → `only:`.
            if !ended && !node.quoted {
                let raw = &text[node.value_start..node.end];
                if let Some(space) = raw.rfind(char::is_whitespace) {
                    let word = raw[space + 1..].to_lowercase();
                    let at = node.value_start + space + 1;
                    choices.extend(args_left(&chain).into_iter().filter(|s| std::iter::once(&s.key).chain(s.aliases).any(|k| k.starts_with(&word))).map(|s| key_row(s, Some(at))));
                }
            }
            choices
        }
    }
}

/// Names chosen in the list, kept by their ids: a name means that one from now on.
#[derive(Debug, Clone, Default)]
pub struct Pins(HashMap<(&'static str, String), Uuid>);

impl Pins {
    pub fn get(&self, kind: Kind, name: &str) -> Option<Uuid> {
        self.0.get(&(kind.pin_tag(), name.trim().to_lowercase())).copied()
    }

    pub fn set(&mut self, kind: Kind, name: &str, id: Uuid) {
        self.0.insert((kind.pin_tag(), name.trim().to_lowercase()), id);
    }
}

/// The line with a row taken: the text it becomes. `space`: a space after a value (Tab;
/// the list goes on to what can follow it).
pub fn take(schema: &Schema, text: &str, row: &Row, space: bool, pins: &mut Pins) -> String {
    let line = parse(schema, text);
    match &row.take {
        Take::Key { key, at: Some(at) } => format!("{}{key}:", &text[..*at]),
        Take::Key { key, at: None } => {
            let base = text.trim_end();
            if base.is_empty() { format!("{key}:") } else { format!("{base} {key}:") }
        }
        Take::Value(value) | Take::Pin(value, _) => {
            let Some(command) = line.current() else { return text.to_string() };
            let node = *command.last_chain().last().unwrap();
            if let Take::Pin(name, id) = &row.take {
                pins.set(node.kind(), name, *id);
            }
            let after = if space { " " } else { "" };
            format!("{}{}{after}", &text[..node.value_start], schema.quoted(value))
        }
    }
}

/// What's wrong with a command, as said in the preview: values that can't be what they
/// are, arguments missing, given twice, or there without the one they go with.
pub fn problems(command: &Node) -> Vec<String> {
    let mut out = Vec::new();
    check(command, &mut out);
    out
}

fn check(node: &Node, out: &mut Vec<String>) {
    let name = node.name();
    let kind = node.kind();
    if node.field.is_none() {
        if kind != Kind::Nothing && node.value.trim().is_empty() && !node.minus {
            out.push(format!("{name}: needs a value"));
        } else if (!node.value.trim().is_empty() || kind == Kind::Nothing)
            && let Err(e) = kind.check(&node.value)
        {
            out.push(format!("{name}: {e}"));
        }
    }
    let allowed = node.allowed();
    for (i, arg) in node.args.iter().enumerate() {
        if arg.field.is_none() && node.args[..i].iter().any(|a| a.field.is_none() && std::ptr::eq(a.spec, arg.spec)) {
            out.push(format!("{}: given twice", arg.name()));
        }
        if let Need::With(other) = arg.spec.need
            && node.arg(other).is_none()
        {
            out.push(format!("{}: only with {other}:", arg.name()));
        }
    }
    let mut groups: Vec<&'static str> = Vec::new();
    for spec in &allowed {
        match spec.need {
            Need::Required if node.arg(spec.key).is_none() => out.push(format!("{name}: needs {}:", spec.key)),
            Need::OneOf(group) if !groups.contains(&group) => {
                groups.push(group);
                let members: Vec<&str> = allowed.iter().filter(|s| s.need == Need::OneOf(group)).map(|s| s.key).collect();
                if !members.iter().any(|k| node.arg(k).is_some()) {
                    out.push(format!("{name}: needs one of {}", members.iter().map(|k| format!("{k}:")).collect::<Vec<_>>().join(", ")));
                }
            }
            _ => {}
        }
    }
    for arg in &node.args {
        check(arg, out);
    }
}

/// Text with one argument of a command set: its value replaced if it's there, else
/// `key:value` added at the end of that command.
pub fn set_arg(schema: &Schema, text: &str, command: usize, key: &str, value: &str) -> String {
    let line = parse(schema, text);
    let Some(c) = line.commands.get(command) else { return text.to_string() };
    let value = schema.quoted(value);
    match c.find(key) {
        Some(arg) => format!("{}{value}{}", &text[..arg.value_start], &text[arg.end..]),
        None => {
            let end = c.last_chain().last().unwrap().end;
            format!("{} {key}:{value}{}", &text[..end], &text[end..])
        }
    }
}

/// Walks every state the list can lead to from an empty line (`depth` rows taken at most),
/// taking every row, and checks that each one taken parses as what it said (a key row
/// leaves that key being typed, a value row that value) and that every declared key is
/// offered somewhere. `lists` stands for the page; `samples` gives values to branch on.
pub fn walk(schema: &Schema, lists: &dyn Lists, samples: &dyn Fn(Kind) -> Vec<String>, depth: usize) -> Result<(), String> {
    let mut offered: Vec<&'static str> = Vec::new();
    let mut todo = vec![(String::new(), 0)];
    let mut seen = std::collections::HashSet::new();
    while let Some((text, d)) = todo.pop() {
        if !seen.insert(text.clone()) || d > depth {
            continue;
        }
        let line = parse(schema, &text);
        let rows = rows(schema, &line, &text, lists);
        // A value done: each sample value of its kind leads on too.
        if let At::Value { chain, .. } = at(&line, &text) {
            let node = chain.last().unwrap();
            if node.value.is_empty() && node.kind() != Kind::Nothing && node.field.is_none() {
                for v in samples(node.kind()) {
                    todo.push((format!("{text}{} ", schema.quoted(&v)), d + 1));
                }
            }
        }
        for row in rows {
            let mut pins = Pins::default();
            let next = take(schema, &text, &row, true, &mut pins);
            let after = parse(schema, &next);
            match &row.take {
                Take::Key { key, .. } => {
                    offered.push(key);
                    let typed = after.current().map(|c| *c.last_chain().last().unwrap());
                    if typed.is_none_or(|n| n.spec.key != *key || !n.value.is_empty()) {
                        return Err(format!("“{text}”: taking {key}: gave “{next}”, which isn't {key}: being typed"));
                    }
                }
                Take::Value(v) | Take::Pin(v, _) => {
                    let typed = after.current().map(|c| *c.last_chain().last().unwrap());
                    if typed.is_none_or(|n| n.value != *v) {
                        return Err(format!("“{text}”: taking “{v}” gave “{next}”, which doesn't have it as the value"));
                    }
                }
            }
            todo.push((next, d + 1));
        }
    }
    for spec in schema.every(samples) {
        if !std::ptr::eq(spec, &FIELD) && !offered.contains(&spec.key) {
            return Err(format!("{}: never offered", spec.key));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAME: &[Word] = &[Word { word: "same", help: "the same one" }];
    const ONLY: &[Word] = &[Word { word: "all", help: "" }, Word { word: "own", help: "" }];

    /// A Credited as…-like line: `artist:` takes `only:`, `to:`, `as:` (one of `to:`/`as:`
    /// needed), `why:`, and `n:`.
    const ARTIST_ARGS: &[Spec] = &[
        Spec::new("only", Kind::Or(ONLY, &Kind::AliasOf(&[])), "which credits"),
        Spec::new("to", Kind::Or(SAME, &Kind::Artist), "to another").need(Need::OneOf("what")),
        Spec::new("as", Kind::Or(&[Word { word: "own", help: "" }], &Kind::AliasOf(&["to"])), "under which name").need(Need::OneOf("what")),
        Spec::new("n", Kind::Place, "where"),
        Spec::new("why", Kind::Text, "kept"),
    ];

    /// A source: by platform, its fields; `-source:` takes `why:`.
    fn platform(value: &str) -> &'static [Spec] {
        const SC: &[Spec] = &[
            Spec::new("song_id", Kind::Text, "").need(Need::OneOf("song")),
            Spec::new("song_uid", Kind::Text, "").need(Need::OneOf("song")),
            Spec::new("album_uuid", Kind::Text, ""),
            Spec::new("index", Kind::Number, "").need(Need::With("album_uuid")),
        ];
        const WHY: &[Spec] = &[Spec::new("why", Kind::Text, "").with_minus()];
        if value.eq_ignore_ascii_case("soundcloud") { SC } else { WHY }
    }

    static SCHEMA: Schema = Schema {
        commands: &[
            Spec::new("artist", Kind::Artist, "whose credits").aliases(&["from"]).next(Next::Args(ARTIST_ARGS)),
            Spec::new("source", Kind::Words(&[Word { word: "soundcloud", help: "" }, Word { word: "web", help: "" }]), "a source").minus(Minus::Off).next(Next::Branch(platform)),
            Spec::new("extra", Kind::Text, "an extra").next(Next::Fields(&[Spec::new("kind", Kind::Words(&[Word { word: "video", help: "" }]), "")])),
            Spec::new("genre", Kind::Text, "").minus(Minus::OffOrClear),
            Spec::new("title", Kind::Text, ""),
        ],
    };

    struct Page;
    const ANN: Uuid = Uuid::from_u128(1);
    const ANN2: Uuid = Uuid::from_u128(2);

    impl Lists for Page {
        fn choices(&self, kind: Kind, typed: &str, _node: &Node, command: &Node) -> Vec<Row> {
            let names: Vec<(&str, Uuid)> = match kind {
                Kind::Artist => vec![("Ann", ANN), ("Ann Lee", ANN2), ("Bo", Uuid::from_u128(3))],
                // Whose names: the `to:` artist's, else the command's.
                Kind::AliasOf(_) => match kind.artist_node(command).map(|n| n.value.as_str()) {
                    Some("Bo") => vec![("Bobby", Uuid::from_u128(30))],
                    _ => vec![("Annie", Uuid::from_u128(10))],
                },
                _ => Vec::new(),
            };
            names.into_iter().filter(|(n, _)| n.to_lowercase().starts_with(&typed.to_lowercase())).map(|(n, id)| Row::pin(n, "", id)).collect()
        }
    }

    fn labels(text: &str) -> Vec<String> {
        rows(&SCHEMA, &parse(&SCHEMA, text), text, &Page).into_iter().map(|r| r.label).collect()
    }

    fn shape(text: &str) -> Vec<String> {
        fn one(n: &Node, out: &mut Vec<String>) {
            out.push(format!("{}{}={}", if n.minus { "-" } else { "" }, n.name(), n.value));
            if !n.args.is_empty() {
                out.push("(".into());
                n.args.iter().for_each(|a| one(a, out));
                out.push(")".into());
            }
        }
        let mut out = Vec::new();
        parse(&SCHEMA, text).commands.iter().for_each(|c| one(c, &mut out));
        out
    }

    #[test]
    fn arguments_come_in_any_order_and_other_keys_are_text() {
        assert_eq!(shape("artist:Ann only:own as:Annie"), ["artist=Ann", "(", "only=own", "as=Annie", ")"]);
        assert_eq!(shape("artist:Ann as:Annie only:own"), ["artist=Ann", "(", "as=Annie", "only=own", ")"]);
        assert_eq!(shape("title:Intro: Part 2 only:x"), ["title=Intro: Part 2 only:x"], "not one of its arguments: text");
        assert_eq!(shape("artist:Ann Lee n:2"), ["artist=Ann Lee", "(", "n=2", ")"]);
        assert_eq!(shape("from:Ann"), ["artist=Ann"], "an alias");
    }

    #[test]
    fn bars_quotes_and_escapes() {
        assert_eq!(shape("artist:A only:x as:y | artist:B to:C"), ["artist=A", "(", "only=x", "as=y", ")", "artist=B", "(", "to=C", ")"]);
        assert_eq!(shape("title:\"a | b\" | title:c \\| d"), ["title=a | b", "title=c | d"]);
        assert_eq!(shape("artist:\"x only:y\" only:z"), ["artist=x only:y", "(", "only=z", ")"]);
        assert_eq!(SCHEMA.quoted("x -genre"), "\"x -genre\"");
        assert_eq!(SCHEMA.quoted("Plain"), "Plain");
        assert_eq!(SCHEMA.quoted("No: Way"), "\"No: Way\"");
        let line = parse(&SCHEMA, "find this | title:x");
        assert_eq!(line.words, ["find", "this"]);
    }

    #[test]
    fn minus_and_clear() {
        assert_eq!(shape("-genre | -genre:rock | -source:web why:gone"), ["-genre=", "-genre=rock", "-source=web", "(", "why=gone", ")"]);
        assert!(parse(&SCHEMA, "-title:x").commands.is_empty(), "title has no minus");
        assert!(parse(&SCHEMA, "-source").commands.is_empty(), "clearing isn't declared");
    }

    #[test]
    fn a_value_decides_what_follows() {
        assert_eq!(shape("source:soundcloud song_id:1 index:2"), ["source=soundcloud", "(", "song_id=1", "index=2", ")"]);
        assert_eq!(shape("source:web song_id:1"), ["source=web song_id:1"], "not the web's");
        assert_eq!(shape("extra:clip kind:video url:https://x.y/z note:a"), ["extra=clip", "(", "kind=video", "url=https://x.y/z", "note=a", ")"], "any name: a field");
        assert_eq!(shape("extra:x http://a.b"), ["extra=x http://a.b"]);
    }

    #[test]
    fn what_is_missing_or_wrong() {
        let problems = |text: &str| -> Vec<String> { parse(&SCHEMA, text).commands.iter().flat_map(problems).collect() };
        assert_eq!(problems("artist:Ann only:own"), ["artist: needs one of to:, as:"]);
        assert!(problems("artist:Ann as:own").is_empty());
        assert_eq!(problems("artist:Ann to:Bo to:Cy"), ["to: given twice"]);
        assert_eq!(problems("artist:Ann to:Bo n:x"), ["n: “x” isn't a place (2, 2-5 or 1+)"]);
        assert_eq!(problems("source:soundcloud index:1"), ["index: only with album_uuid:", "source: needs one of song_id:, song_uid:"]);
        assert!(problems("source:soundcloud song_uid:x album_uuid:y index:1").is_empty());
        assert_eq!(problems("source:nowhere"), ["source: “nowhere” isn't one of: soundcloud, web"]);
        assert_eq!(problems("artist: as:own"), ["artist: needs a value"]);
        assert!(problems("-genre").is_empty());
    }

    #[test]
    fn places() {
        assert_eq!(place("2"), Some(Place { disc: None, n: 2, in_order: false }));
        assert_eq!(place("2-5"), Some(Place { disc: Some(2), n: 5, in_order: false }));
        assert_eq!(place("1+"), Some(Place { disc: None, n: 1, in_order: true }));
        assert_eq!(place("#2"), None, "no old syntax");
        assert_eq!(place("x"), None);
    }

    #[test]
    fn the_list_follows_the_table() {
        // Nothing typed, or after a bar: the commands (those not used first).
        assert_eq!(labels(""), ["artist:", "source:", "extra:", "genre:", "title:"]);
        assert_eq!(labels("artist:Ann as:own | "), ["source:", "extra:", "genre:", "title:", "artist:"]);
        assert_eq!(labels("artist:Ann as:own |"), ["source:", "extra:", "genre:", "title:", "artist:"]);
        // A word: the commands it starts (aliases too).
        assert_eq!(labels("ar"), ["artist:"]);
        assert_eq!(labels("fr"), ["artist:"]);
        // In a value: its choices.
        assert_eq!(labels("artist:"), ["Ann", "Ann Lee", "Bo"]);
        assert_eq!(labels("artist:An"), ["Ann", "Ann Lee"]);
        // A space inside a name: the choices go on while one could.
        assert_eq!(labels("artist:Ann "), ["to:", "as:", "only:", "n:", "why:"], "exactly a choice: done");
        assert_eq!(labels("artist:Ann L"), ["Ann Lee"]);
        assert_eq!(labels("artist:Zed "), ["to:", "as:", "only:", "n:", "why:"], "none goes on: done");
        // Done: the arguments left, needed ones first.
        assert_eq!(labels("artist:Ann only:own "), ["to:", "as:", "n:", "why:"]);
        assert_eq!(labels("artist:Ann only:"), ["all", "own", "Annie"], "words, then the page's");
        // The last word of a value can be a key being typed.
        assert_eq!(labels("artist:Ann on"), ["only:"]);
        // `to:Bo as:` lists Bo's names; without `to:`, the artist's.
        assert_eq!(labels("artist:Ann to:Bo as:"), ["own", "Bobby"]);
        assert_eq!(labels("artist:Ann as:"), ["own", "Annie"]);
        // Branch: the platform's fields.
        assert_eq!(labels("source:soundcloud "), ["song_id:", "song_uid:", "album_uuid:", "index:"]);
        assert_eq!(labels("-source:web "), ["why:"]);
        // Clearing: nothing to choose, what follows.
        assert!(labels("-genre").is_empty());
    }

    #[test]
    fn taking_a_row_never_writes_the_next_key() {
        let mut pins = Pins::default();
        let take_label = |text: &str, label: &str, space: bool, pins: &mut Pins| -> String {
            let row = rows(&SCHEMA, &parse(&SCHEMA, text), text, &Page).into_iter().find(|r| r.label == label).unwrap();
            take(&SCHEMA, text, &row, space, pins)
        };
        assert_eq!(take_label("artist:An", "Ann Lee", true, &mut pins), "artist:Ann Lee ");
        assert_eq!(pins.get(Kind::Artist, "ann lee"), Some(ANN2), "pinned by its id");
        assert_eq!(take_label("artist:An", "Ann", false, &mut pins), "artist:Ann");
        assert_eq!(take_label("artist:Ann ", "only:", true, &mut pins), "artist:Ann only:");
        assert_eq!(take_label("artist:Ann on", "only:", true, &mut pins), "artist:Ann only:");
        assert_eq!(take_label("ar", "artist:", true, &mut pins), "artist:");
        assert_eq!(take_label("artist:Ann as:own | ", "artist:", true, &mut pins), "artist:Ann as:own | artist:");
        assert_eq!(take_label("artist:Ann only:", "own", true, &mut pins), "artist:Ann only:own ");
    }

    #[test]
    fn set_an_argument() {
        assert_eq!(set_arg(&SCHEMA, "artist:Ann only:own", 0, "only", "all"), "artist:Ann only:all");
        assert_eq!(set_arg(&SCHEMA, "artist:Ann | artist:Bo", 0, "as", "A: B"), "artist:Ann as:\"A: B\" | artist:Bo");
        assert_eq!(set_arg(&SCHEMA, "artist:Ann | artist:Bo ", 1, "to", "same"), "artist:Ann | artist:Bo to:same ");
    }

    #[test]
    fn every_state_leads_somewhere_valid() {
        let samples = |k: Kind| -> Vec<String> {
            match k.looked_up() {
                Kind::Words(w) => w.iter().map(|w| w.word.to_string()).collect(),
                Kind::Text => vec!["x".into()],
                _ => Vec::new(),
            }
        };
        walk(&SCHEMA, &Page, &samples, 4).unwrap();
    }
}
