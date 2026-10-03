//! A parse's setting, as typed after `parse:` (and saved, as the same text): the rules,
//! then the overrides, each running until the next one.
//!
//! ```text
//! feat prod [prod] with=prod split:-x -and whole:off keep:cdq -live word:vs=featured
//! ```
//!
//! * Rules: `prod`, `feat` (`ft`), `with` (`w/`), `w`, `x`, `prefix`, each in its own role
//!   or another (`with=prod`); `+feat` / `+prod` (`+=role`, the role is needed); `[prod]`,
//!   `(prod)`, `{prod}` (`[]=role`; `[]` alone: producer). Roles as the vocabulary has
//!   them, any start of one.
//! * `split:` the separators names are split on: only these (`split:, &`), or the default
//!   without some (`split:-x -and`).
//! * `whole:off`: names aren't kept whole (see `parse`).
//! * `keep:` words that are never names in a bracket, added (`keep:cdq`) or taken off the
//!   default (`keep:-live`). `"…"` for more than one word.
//! * `word:<keyword>=role`: a keyword of your own, working like the others
//!   (`word:"beat by"=producer`).

/// The rules as they're offered while typing: (as typed, what it finds).
pub const RULES: &[(&str, &str)] = &[
    ("feat", "feat. ft. featuring → featured"),
    ("prod", "prod. produced by → producer"),
    ("with", "w/ with → featured"),
    ("w", "w alone → featured"),
    ("x", "x alone → featured"),
    ("+feat", "+ Name → featured"),
    ("+prod", "+ Name → producer"),
    ("[prod]", "[Name] → producer"),
    ("[feat]", "[Name] → featured"),
    ("(prod)", "(Name) → producer"),
    ("(feat)", "(Name) → featured"),
    ("prefix", "Name - Title → primary"),
];

/// Separators names are split on by default.
pub const SPLIT: &[&str] = &[",", "&", "x", "+", "and"];

/// Words that are never names by default: a bracket with one of them in it stays.
pub const KEEP: &[&str] = &[
    "remix", "rmx", "mix", "edit", "version", "ver", "instrumental", "inst", "acapella", "acappella", "a cappella", "demo",
    "snippet", "preview", "leak", "leaked", "og", "original", "extended", "slowed", "reverb", "sped", "nightcore", "chopped",
    "screwed", "8d", "live", "acoustic", "remaster", "remastered", "mastered", "unmastered", "mixed", "unmixed", "cdq", "hq",
    "lq", "explicit", "clean", "radio", "intro", "outro", "interlude", "skit", "reprise", "bootleg", "cover", "freestyle",
    "vip", "remake", "rework", "flip", "unreleased", "video", "audio", "official", "lyrics", "visualizer", "mono", "stereo",
    "alt", "alternate", "final", "session",
];

const KEYS: &[&str] = &["split", "whole", "keep", "word"];

/// A bracket's shape (the full-width ones count as theirs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Square,
    Round,
    Curly,
}

impl Shape {
    const ALL: [Shape; 3] = [Shape::Square, Shape::Round, Shape::Curly];

    fn chars(self) -> (char, char) {
        match self {
            Shape::Square => ('[', ']'),
            Shape::Round => ('(', ')'),
            Shape::Curly => ('{', '}'),
        }
    }
}

/// What a rule looks for. The order here is the order they run in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Prod,
    Feat,
    With,
    W,
    X,
    /// A keyword of the user's own (lowercase, single spaces).
    Word(String),
    Plus,
    Bracket(Shape),
    Prefix,
}

impl Kind {
    fn rank(&self) -> u8 {
        match self {
            Kind::Prod => 0,
            Kind::Feat => 1,
            Kind::With => 2,
            Kind::W => 3,
            Kind::X => 4,
            Kind::Word(_) => 5,
            Kind::Plus => 6,
            Kind::Bracket(_) => 7,
            Kind::Prefix => 8,
        }
    }

    /// A keyword rule's words (none for the others).
    pub fn keywords(&self) -> Vec<&str> {
        match self {
            Kind::Prod => vec!["produced by", "prod. by", "prod by", "prod.", "prod"],
            Kind::Feat => vec!["featuring", "feat.", "feat", "ft.", "ft"],
            Kind::With => vec!["with", "w/"],
            Kind::W => vec!["w"],
            Kind::X => vec!["x"],
            Kind::Word(w) => vec![w.as_str()],
            Kind::Plus | Kind::Bracket(_) | Kind::Prefix => vec![],
        }
    }

    /// The name of a rule typed as a word.
    fn name(&self) -> Option<&'static str> {
        Some(match self {
            Kind::Prod => "prod",
            Kind::Feat => "feat",
            Kind::With => "with",
            Kind::W => "w",
            Kind::X => "x",
            Kind::Prefix => "prefix",
            _ => return None,
        })
    }

    fn default_role(&self) -> Option<&'static str> {
        match self {
            Kind::Prod | Kind::Bracket(_) => Some("producer"),
            Kind::Feat | Kind::With | Kind::W | Kind::X => Some("featured"),
            Kind::Prefix => Some("primary"),
            Kind::Word(_) | Kind::Plus => None,
        }
    }
}

/// A rule, in the role what it finds is credited in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub kind: Kind,
    pub role: String,
}

impl Rule {
    /// As it's written in a setting (`feat`, `with=producer`, `+featured`, `[producer]`,
    /// `word:vs=featured`).
    pub fn token(&self) -> String {
        match &self.kind {
            Kind::Word(w) => format!("word:{}={}", quoted(w), self.role),
            Kind::Plus => format!("+{}", self.role),
            Kind::Bracket(shape) => {
                let (o, c) = shape.chars();
                format!("{o}{}{c}", self.role)
            }
            kind => match kind.default_role() {
                Some(d) if d == self.role => kind.name().unwrap_or_default().to_string(),
                _ => format!("{}={}", kind.name().unwrap_or_default(), self.role),
            },
        }
    }
}

/// The rules (in the order they run) and the overrides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    pub rules: Vec<Rule>,
    /// The separators names are split on (`,` alone; the others need spaces around).
    pub split: Vec<String>,
    /// Whether pieces that are an artist's name together are kept whole.
    pub whole: bool,
    /// Words that are never names in a bracket.
    pub keep: Vec<String>,
}

impl Setting {
    /// `text` read (see the module's notes). `role`: the role a typed one means, if any.
    pub fn parse(text: &str, role: &dyn Fn(&str) -> Option<String>) -> Result<Setting, String> {
        let role_of = |typed: &str| -> Result<String, String> {
            if typed.is_empty() {
                return Err("which role? (after the =)".into());
            }
            role(typed).ok_or_else(|| format!("no role like “{typed}”"))
        };
        let mut rules: Vec<Rule> = Vec::new();
        let (mut only, mut without): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
        let mut whole = None;
        let (mut keep_add, mut keep_off): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
        let mut key = None;
        for token in tokens(text) {
            let value = match token.split_once(':') {
                Some((k, v)) if KEYS.contains(&k.to_lowercase().as_str()) => {
                    key = Some(k.to_lowercase());
                    if v.is_empty() {
                        continue;
                    }
                    v.to_string()
                }
                _ => token,
            };
            let lower = value.to_lowercase();
            match key.as_deref() {
                None => {
                    let rule = rule(&lower, &role_of)?;
                    if rules.iter().any(|r| r.kind == rule.kind) {
                        return Err(format!("“{value}”: that rule is there already"));
                    }
                    rules.push(rule);
                }
                Some("split") => match lower.strip_prefix('-') {
                    Some(s) if !s.is_empty() => {
                        if !SPLIT.contains(&s) {
                            return Err(format!("“{s}” isn't split on by default ({})", SPLIT.join(" ")));
                        }
                        without.push(s.to_string());
                    }
                    _ => only.push(lower),
                },
                Some("whole") => {
                    if whole.is_some() {
                        return Err("whole: on or off, once".into());
                    }
                    whole = Some(match lower.as_str() {
                        "on" | "yes" | "true" => true,
                        "off" | "no" | "false" => false,
                        _ => return Err(format!("whole: on or off, not “{value}”")),
                    });
                }
                Some("keep") => match lower.strip_prefix('-') {
                    Some(w) if !w.is_empty() => {
                        let w = spaced(w);
                        if !KEEP.contains(&w.as_str()) {
                            return Err(format!("“{w}” isn't one of the words kept by default"));
                        }
                        keep_off.push(w);
                    }
                    _ => keep_add.push(spaced(&lower)),
                },
                Some(_) => {
                    let (word, typed) = lower.rsplit_once('=').ok_or_else(|| format!("word:{value} needs a role: word:{value}=featured"))?;
                    let word = spaced(word);
                    if word.is_empty() {
                        return Err("word: which keyword? (word:vs=featured)".into());
                    }
                    let rule = Rule { kind: Kind::Word(word), role: role_of(typed)? };
                    if rules.iter().any(|r| r.kind == rule.kind) {
                        return Err(format!("word:{value}: that keyword is there already"));
                    }
                    rules.push(rule);
                }
            }
        }
        if rules.is_empty() {
            return Err("which rules? feat, prod, with, w, x, +feat, [prod], prefix…".into());
        }
        if !only.is_empty() && !without.is_empty() {
            return Err("split: either the separators to split on (split:, &) or the default without some (split:-x -and), not both".into());
        }
        rules.sort_by_key(|r| r.kind.rank());
        let mut split: Vec<String> = Vec::new();
        for s in if only.is_empty() { SPLIT.iter().map(|s| s.to_string()).filter(|s| !without.contains(s)).collect() } else { only } {
            if !split.contains(&s) {
                split.push(s);
            }
        }
        let mut keep: Vec<String> = KEEP.iter().map(|s| s.to_string()).filter(|w| !keep_off.contains(w)).collect();
        for w in keep_add {
            if !keep.contains(&w) {
                keep.push(w);
            }
        }
        Ok(Setting { rules, split, whole: whole.unwrap_or(true), keep })
    }

    /// As text that reads back the same: the rules, then what differs from the defaults.
    pub fn to_text(&self) -> String {
        let mut parts: Vec<String> = self.rules.iter().filter(|r| !matches!(r.kind, Kind::Word(_))).map(Rule::token).collect();
        let default: Vec<&str> = SPLIT.to_vec();
        if self.split != default {
            let kept: Vec<&str> = default.iter().copied().filter(|s| self.split.iter().any(|x| x == s)).collect();
            if kept == self.split {
                let off: Vec<String> = default.iter().filter(|s| !kept.contains(s)).map(|s| format!("-{s}")).collect();
                parts.push(format!("split:{}", off.join(" ")));
            } else {
                parts.push(format!("split:{}", self.split.iter().map(|s| quoted(s)).collect::<Vec<_>>().join(" ")));
            }
        }
        if !self.whole {
            parts.push("whole:off".into());
        }
        let off: Vec<String> = KEEP.iter().filter(|w| !self.keep.iter().any(|k| k == *w)).map(|w| format!("-{}", quoted(w))).collect();
        let added: Vec<String> = self.keep.iter().filter(|k| !KEEP.contains(&k.as_str())).map(|k| quoted(k)).collect();
        if !off.is_empty() || !added.is_empty() {
            parts.push(format!("keep:{}", added.into_iter().chain(off).collect::<Vec<_>>().join(" ")));
        }
        parts.extend(self.rules.iter().filter(|r| matches!(r.kind, Kind::Word(_))).map(Rule::token));
        parts.join(" ")
    }

    /// Every keyword rule's words: where credits stop.
    pub fn keywords(&self) -> Vec<&str> {
        self.rules.iter().flat_map(|r| r.kind.keywords()).collect()
    }
}

/// A rule typed (lowercase).
fn rule(token: &str, role_of: &dyn Fn(&str) -> Result<String, String>) -> Result<Rule, String> {
    let unknown = || format!("no rule like “{token}” (feat, prod, with, w, x, +feat, +prod, [prod], (prod), {{prod}}, prefix, word:…=role)");
    if let Some(rest) = token.strip_prefix('+') {
        let typed = rest.strip_prefix('=').unwrap_or(rest);
        if typed.is_empty() {
            return Err("+ needs a role: +feat or +prod".into());
        }
        return Ok(Rule { kind: Kind::Plus, role: role_of(typed)? });
    }
    for shape in Shape::ALL {
        let (o, c) = shape.chars();
        if let Some(rest) = token.strip_prefix(o) {
            let role = if let Some(typed) = rest.strip_prefix(c).and_then(|r| r.strip_prefix('=')) {
                role_of(typed)?
            } else if rest == c.to_string() {
                "producer".to_string()
            } else if let Some(typed) = rest.strip_suffix(c) {
                role_of(typed)?
            } else {
                return Err(unknown());
            };
            return Ok(Rule { kind: Kind::Bracket(shape), role });
        }
    }
    let (name, typed) = match token.split_once('=') {
        Some((n, r)) => (n, Some(r)),
        None => (token, None),
    };
    let kind = match name {
        "prod" => Kind::Prod,
        "feat" | "ft" => Kind::Feat,
        "with" | "w/" => Kind::With,
        "w" => Kind::W,
        "x" => Kind::X,
        "prefix" => Kind::Prefix,
        _ => return Err(unknown()),
    };
    let role = match typed {
        Some(typed) => role_of(typed)?,
        None => kind.default_role().unwrap_or_default().to_string(),
    };
    Ok(Rule { kind, role })
}

/// Whitespace-separated, `"…"` keeping spaces in.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let (mut quoted, mut any) = (false, false);
    for c in text.chars() {
        if c == '"' {
            quoted = !quoted;
            any = true;
        } else if c.is_whitespace() && !quoted {
            if any || !current.is_empty() {
                out.push(std::mem::take(&mut current));
                any = false;
            }
        } else {
            current.push(c);
        }
    }
    if any || !current.is_empty() {
        out.push(current);
    }
    out
}

/// Single spaces, trimmed.
fn spaced(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn quoted(s: &str) -> String {
    if s.contains(char::is_whitespace) { format!("\"{s}\"") } else { s.to_string() }
}
