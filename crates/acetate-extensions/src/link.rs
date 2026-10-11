//! Links: reading a pasted one with a template, and filling a template in.
//!
//! A template is a link with `{name}` where a part is kept: `https://youtu.be/{song_id}`,
//! `https://soundcloud.com/{user}/sets/{set}`, `https://music.163.com/#/song?id={song_id}`.
//! The scheme, `www.` and `m.` don't matter. The path has the same parts in the same places
//! (a part may have text around it: `@{channel}`); a last `*` is anything more. A query
//! parameter in the template has to be in the link (others are ignored), and so does a
//! fragment, read like a path and query of its own.

use std::collections::HashMap;

use serde_json::Value;

use crate::{Link, Manifest};

/// What a link says: the platform, the kind of thing, and the fields of its source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkMatch {
    pub platform: String,
    pub kind: String,
    pub fields: Vec<(String, String)>,
}

/// A path's segments and a query's name/value pairs.
type PathQuery = (Vec<String>, Vec<(String, String)>);

/// A link taken apart.
pub(crate) struct Parts {
    host: String,
    segments: Vec<String>,
    query: Vec<(String, String)>,
    fragment: Option<PathQuery>,
}

fn path_and_query(text: &str) -> PathQuery {
    let (path, query) = text.split_once('?').unwrap_or((text, ""));
    let segments = path.split('/').filter(|s| !s.is_empty()).map(str::to_string).collect();
    let query = query.split('&').filter_map(|pair| pair.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
    (segments, query)
}

impl Parts {
    pub(crate) fn of(link: &str) -> Option<Parts> {
        let link = link.trim();
        if link.is_empty() || link.contains(char::is_whitespace) {
            return None;
        }
        let rest = link.strip_prefix("https://").or_else(|| link.strip_prefix("http://")).unwrap_or(link);
        let (host, rest) = rest.split_once('/').unwrap_or((rest, ""));
        if host.is_empty() || !host.contains('.') {
            return None;
        }
        let host = host.to_lowercase();
        let host = host.strip_prefix("www.").or_else(|| host.strip_prefix("m.")).unwrap_or(&host).to_string();
        let (before, fragment) = match rest.split_once('#') {
            Some((before, fragment)) => (before, Some(fragment)),
            None => (rest, None),
        };
        let (segments, query) = path_and_query(before);
        Some(Parts { host, segments, query, fragment: fragment.map(path_and_query) })
    }

    pub(crate) fn host(&self) -> &str {
        &self.host
    }

    fn path_text(&self) -> String {
        self.segments.join("/")
    }
}

/// One part of a template path or query value: text, a kept part, text.
struct Piece {
    before: String,
    name: Option<String>,
    after: String,
}

impl Piece {
    fn of(text: &str) -> Piece {
        match (text.find('{'), text.find('}')) {
            (Some(open), Some(close)) if open < close => Piece { before: text[..open].into(), name: Some(text[open + 1..close].into()), after: text[close + 1..].into() },
            _ => Piece { before: text.into(), name: None, after: String::new() },
        }
    }

    /// What of `text` the part keeps (Some("") for text with no part in it that is equal).
    fn read<'a>(&self, text: &'a str) -> Option<&'a str> {
        let inner = text.strip_prefix(self.before.as_str())?.strip_suffix(self.after.as_str())?;
        match self.name {
            Some(_) => Some(inner),
            None => inner.is_empty().then_some(""),
        }
    }
}

struct Pattern {
    host: String,
    segments: Vec<Piece>,
    rest: bool,
    query: Vec<(String, Piece)>,
    fragment: Option<Box<Pattern>>,
}

fn pattern(template: &str) -> Result<Pattern, String> {
    let rest = template.strip_prefix("https://").or_else(|| template.strip_prefix("http://")).unwrap_or(template);
    let (host, rest) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    let (before, fragment) = match rest.split_once('#') {
        Some((before, fragment)) => (before, Some(fragment)),
        None => (rest, None),
    };
    let mut pattern = path_pattern(before)?;
    pattern.host = host;
    pattern.fragment = fragment.map(path_pattern).transpose()?.map(Box::new);
    Ok(pattern)
}

fn path_pattern(text: &str) -> Result<Pattern, String> {
    let (path, query) = text.split_once('?').unwrap_or((text, ""));
    let mut pieces: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let rest = pieces.last() == Some(&"*");
    if rest {
        pieces.pop();
    }
    let query = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| pair.split_once('=').map(|(k, v)| (k.to_string(), Piece::of(v))).ok_or_else(|| format!("“{pair}” in a link template is name=value")))
        .collect::<Result<_, _>>()?;
    Ok(Pattern { host: String::new(), segments: pieces.into_iter().map(Piece::of).collect(), rest, query, fragment: None })
}

fn captures<'a>(pattern: &Pattern, segments: &'a [String], query: &'a [(String, String)], into: &mut Vec<(String, &'a str)>) -> Option<()> {
    if segments.len() < pattern.segments.len() || (!pattern.rest && segments.len() != pattern.segments.len()) {
        return None;
    }
    for (piece, text) in pattern.segments.iter().zip(segments) {
        let kept = piece.read(text)?;
        if let Some(name) = &piece.name {
            into.push((name.clone(), kept));
        }
    }
    for (key, piece) in &pattern.query {
        let (_, value) = query.iter().find(|(k, _)| k == key)?;
        let kept = piece.read(value)?;
        if let Some(name) = &piece.name {
            into.push((name.clone(), kept));
        }
    }
    Some(())
}

/// The names a template keeps.
fn names(pattern: &Pattern) -> Vec<&str> {
    let mut out: Vec<&str> = pattern.segments.iter().filter_map(|p| p.name.as_deref()).chain(pattern.query.iter().filter_map(|(_, p)| p.name.as_deref())).collect();
    if let Some(fragment) = &pattern.fragment {
        out.extend(names(fragment));
    }
    out
}

/// The host a template is for (`www.` left off).
pub(crate) fn template_host(template: &str) -> String {
    pattern(template).map(|p| p.host).unwrap_or_default()
}

/// Whether the template is well made, for `Manifest::check`.
pub(crate) fn check(manifest: &Manifest, link: &Link) -> Result<(), String> {
    if link.custom.is_some() {
        if link.hosts.is_empty() {
            return Err("a custom link needs hosts".into());
        }
        return Ok(());
    }
    if link.matches.is_empty() {
        return Err(format!("a {} link needs something to match", link.kind));
    }
    let shape = manifest.shapes.iter().find(|s| s.kind == link.kind).ok_or_else(|| format!("a {} link, and no {} shape", link.kind, link.kind))?;
    for template in &link.matches {
        let p = pattern(template).map_err(|e| format!("{template}: {e}"))?;
        let kept = names(&p);
        for name in link.set.keys() {
            if !shape.fields.iter().any(|f| &f.name == name) {
                return Err(format!("{template}: the shape has no field “{name}”"));
            }
        }
        if link.set.is_empty() {
            if let Some(name) = kept.iter().find(|n| !shape.fields.iter().any(|f| &f.name == *n)) {
                return Err(format!("{template}: “{name}” is no field of the shape (say what it makes with `set`)"));
            }
        } else if link.set.values().all(|t| fill(t, |_| Some(String::new())).is_none()) {
            return Err(format!("{template}: `set` is empty"));
        }
    }
    Ok(())
}

/// A template with `{name}` filled by `value`; None if one has no value.
pub(crate) fn fill(template: &str, value: impl Fn(&str) -> Option<String>) -> Option<String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let close = rest[open..].find('}')? + open;
        out.push_str(&rest[..open]);
        out.push_str(&value(&rest[open + 1..close])?);
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// A data value as text for a link: a list gives its last (newest) item.
pub(crate) fn value_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Array(items) => value_text(items.last()?),
        _ => None,
    }
}

fn host_is(host: &str, wanted: &str) -> bool {
    match wanted.strip_suffix(".*") {
        Some(start) => host.starts_with(&format!("{start}.")),
        None => host == wanted || host.ends_with(&format!(".{wanted}")),
    }
}

/// What `link` (taken apart as `parts`) is, by one of `extension`'s link rules.
pub(crate) fn read(extension: &Manifest, rule: &Link, parts: &Parts, link: &str) -> Option<LinkMatch> {
    let platform = extension.extension.name.clone();
    if let Some(label) = &rule.custom {
        if !rule.hosts.iter().any(|h| host_is(&parts.host, h)) || rule.path_contains.as_ref().is_some_and(|p| !parts.path_text().contains(p.as_str())) {
            return None;
        }
        let link = link.trim();
        let url = if link.contains("://") { link.to_string() } else { format!("https://{link}") };
        return Some(LinkMatch { platform: "custom".into(), kind: rule.kind.clone(), fields: vec![("label".into(), label.clone()), ("url".into(), url)] });
    }
    for template in &rule.matches {
        let Ok(p) = pattern(template) else { continue };
        if p.host != parts.host {
            continue;
        }
        let mut found = Vec::new();
        if captures(&p, &parts.segments, &parts.query, &mut found).is_none() {
            continue;
        }
        if let Some(fragment) = &p.fragment {
            let Some((segments, query)) = &parts.fragment else { continue };
            if captures(fragment, segments, query, &mut found).is_none() {
                continue;
            }
        }
        let kept: HashMap<&str, &str> = found.iter().map(|(n, t)| (n.as_str(), *t)).collect();
        if !kept.iter().all(|(name, text)| rule.captures.get(*name).is_none_or(|c| c.allows(text))) {
            continue;
        }
        let fields: Vec<(String, String)> = if rule.set.is_empty() {
            found.iter().map(|(n, t)| (n.clone(), t.to_string())).collect()
        } else {
            let mut set: Vec<_> = rule.set.iter().collect();
            set.sort();
            set.into_iter().filter_map(|(field, t)| Some((field.clone(), fill(t, |name| kept.get(name).map(|s| s.to_string()))?))).collect()
        };
        if !fields.is_empty() {
            return Some(LinkMatch { platform, kind: rule.kind.clone(), fields });
        }
    }
    None
}
