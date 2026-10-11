//! `source:` as typed: a link, a platform and its fields, checked against the shapes the server describes.

use serde_json::Value;

use crate::account::{ShapeField, SourceShape};

/// What `source:` typed says (a commandline's: the track editor's, the artist page's).
pub struct Typed {
    pub platform: String,
    pub data: Value,
    /// What tells it apart (`editor::model::source_name`).
    pub name: String,
    /// In words: "netease 123", "custom “vk”", or (typed field by field) its fields.
    pub shown: String,
}

/// `source:`'s value (a link, `custom label, description`, or a platform) and the fields
/// typed after it (name, value; they win over what the value says), as a `kind` of source
/// ("track", "album", "artist").
pub fn typed(value: &str, fields: &[(&str, &str)], kind: &str, shapes: &[SourceShape]) -> Result<Typed, String> {
    let (first, rest) = value.split_once(char::is_whitespace).map(|(f, r)| (f, r.trim())).unwrap_or((value, ""));
    let typed_shape = shapes.iter().find(|s| s.kind == kind && s.platform.eq_ignore_ascii_case(first));
    // What the value says (a link, `custom vk, …`); the fields after it add to it.
    let (platform, said, by_fields) = match typed_shape {
        // `source:custom vk, sent by a friend`: the label, then a comma and more.
        Some(shape) if shape.platform == "custom" && !rest.is_empty() => {
            let (label, description) = rest.split_once(',').map(|(l, d)| (l.trim(), d.trim())).unwrap_or((rest, ""));
            ("custom".to_string(), vec![("label".to_string(), label.to_string()), ("description".to_string(), description.to_string())], !fields.is_empty())
        }
        Some(shape) if shape.platform == "custom" && fields.is_empty() => {
            return Err("source:custom and where it's from (a label: custom vk), then a comma and more if there's more".into());
        }
        // `source:distrokid album_uuid:… index:1`: field by field.
        Some(shape) if rest.is_empty() => (shape.platform.clone(), Vec::new(), true),
        Some(shape) => return Err(format!("“{rest}”: after source:{}, its fields: {}", shape.platform, field_list(shape))),
        None => {
            let (platform, values) = from_link(value, kind).ok_or_else(|| "source: a link (https://…), a platform and its fields (distrokid album_uuid:… index:1), or custom and where it's from".to_string())?;
            (platform, values, !fields.is_empty())
        }
    };
    let shape = shapes.iter().find(|s| s.platform == platform && s.kind == kind).ok_or_else(|| format!("no {platform} sources for {kind}s"))?;
    let values = typed_fields(shape, said, fields)?;
    let data = data_from(shape, &values).map_err(|err| if by_fields { format!("{err} source:{platform} {}", field_list(shape)) } else { err })?;
    let id_value = shape.id_field.as_ref().and_then(|field| data.get(field)).map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()));
    let name = super::model::source_name(id_value.as_deref(), &data);
    // Typed field by field: every field shown, as typed back (a typo shows).
    let shown = if by_fields {
        format!("{platform} {}", fields_typed(&data))
    } else {
        match (platform.as_str(), name.as_str()) {
            (p, "") => p.to_string(),
            ("custom", name) => format!("custom “{name}”"),
            (p, name) => format!("{p} {name}"),
        }
    };
    Ok(Typed { platform, data, name, shown })
}

/// One of the platform's fields, by name as typed (any case).
pub fn field_named<'a>(shape: &'a SourceShape, name: &str) -> Option<&'a ShapeField> {
    shape.fields.iter().find(|f| f.name.eq_ignore_ascii_case(name))
}

/// Each of the platform's fields and its text, as the form takes them: what the value said
/// (`said`: a link's, `custom vk`'s), and the fields typed after it (`fields`, which win).
/// A list's field is given again for each item.
fn typed_fields(shape: &SourceShape, said: Vec<(String, String)>, fields: &[(&str, &str)]) -> Result<Vec<(String, String)>, String> {
    let mut given: Vec<(&ShapeField, &str)> = Vec::new();
    for &(name, value) in fields {
        let field = field_named(shape, name).ok_or_else(|| format!("{name}: {} sources have no such field; theirs: {}", shape.platform, field_list(shape)))?;
        given.push((field, value));
    }
    let mut values = Vec::new();
    for field in &shape.fields {
        let texts: Vec<&str> = given.iter().filter(|(f, _)| f.name == field.name).map(|(_, t)| *t).collect();
        if texts.len() > 1 && field.kind != "strs" {
            return Err(format!("{}: twice (it's one value)", field.name));
        }
        let text = if texts.is_empty() { said.iter().find(|(n, _)| *n == field.name).map(|(_, v)| v.clone()).unwrap_or_default() } else { texts.join("\n") };
        values.push((field.name.clone(), text));
    }
    Ok(values)
}

/// The next of the platform's fields to type, after `given` (names as typed): the first
/// it needs, else the first it may have. None: every one is there.
pub fn next_field<'a>(shape: &'a SourceShape, given: &[&str]) -> Option<&'a ShapeField> {
    let missing = || shape.fields.iter().filter(|f| !given.iter().any(|g| f.name.eq_ignore_ascii_case(g)));
    missing().find(|f| f.required).or_else(|| missing().next())
}

/// The platform `source:`'s value names (typed, or a link's), for its fields.
pub fn shape_of<'a>(value: &str, kind: &str, shapes: &'a [SourceShape]) -> Option<&'a SourceShape> {
    let value = value.trim().trim_start_matches('=').trim();
    let first = value.split_whitespace().next()?;
    let platform = match shapes.iter().find(|s| s.kind == kind && s.platform.eq_ignore_ascii_case(first)) {
        Some(shape) => return Some(shape),
        None => from_link(value, kind)?.0,
    };
    shapes.iter().find(|s| s.kind == kind && s.platform == platform)
}

/// The platform's fields, as typed: `album_uuid:… (needed) index:…`.
pub fn field_list(shape: &SourceShape) -> String {
    shape.fields.iter().map(|f| format!("{}:…{}", f.name, if f.required { " (needed)" } else { "" })).collect::<Vec<_>>().join(" ")
}

/// A source's data as `source:` takes it back: `album_uuid:… index:1` (a list: its field
/// once per item).
pub fn fields_typed(data: &serde_json::Value) -> String {
    let Some(fields) = data.as_object() else { return String::new() };
    let mut typed = Vec::new();
    for (name, value) in fields {
        match value {
            serde_json::Value::String(s) => typed.push(format!("{name}:{s}")),
            serde_json::Value::Array(items) => typed.extend(items.iter().map(|i| format!("{name}:{}", i.as_str().map(str::to_string).unwrap_or_else(|| i.to_string())))),
            other => typed.push(format!("{name}:{other}")),
        }
    }
    typed.join(" ")
}

/// A choice under `source:` (a platform) or one of its fields, for a commandline's list.
pub struct Choice {
    pub label: String,
    /// What it is, what it takes.
    pub sub: String,
    /// The value it puts in.
    pub text: String,
    /// What Tab writes after it: the platform's first field, the next field.
    pub next: Option<String>,
}

/// The platforms starting with `typed`, each with its fields to type after it (`before`:
/// put before the name, `=` for `source:=…`). Tab adds the first field (`distrokid
/// album_uuid:`), as `artist:` adds `role:`; custom takes its label as it is (`custom vk`).
pub fn platform_choices(typed: &str, before: &str, kind: &str, shapes: &[SourceShape]) -> Vec<Choice> {
    shapes
        .iter()
        .filter(|s| s.kind == kind && s.platform.starts_with(&typed.to_lowercase()))
        .map(|shape| {
            let (sub, next) = match shape.platform.as_str() {
                "custom" => ("where it's from, then a comma and more: custom vk, sent by a friend".to_string(), None),
                _ => (field_list(shape), next_field(shape, &[]).map(|f| format!("{}:", f.name))),
            };
            Choice { label: format!("{before}{}", shape.platform), sub, text: format!("{before}{}", shape.platform), next }
        })
        .collect()
}

/// A source's field being typed (`name`, its `value` so far; `given`: the fields typed,
/// this one too), after `source:` with `platform_value`: what it takes, and on Tab the next
/// field (needed ones first). Not one of the platform's fields: the ones it has.
pub fn field_choices(platform_value: &str, name: &str, value: &str, given: &[&str], kind: &str, shapes: &[SourceShape]) -> Vec<Choice> {
    let Some(shape) = shape_of(platform_value, kind, shapes) else { return Vec::new() };
    let needed = |f: &ShapeField| if f.required { " (needed)" } else { "" };
    match field_named(shape, name) {
        Some(field) => {
            let what = match field.kind.as_str() {
                "int" => "a number",
                "strs" => "text; the field again for another",
                _ => "text",
            };
            // (Nothing typed yet: Tab doesn't skip it.)
            let then = next_field(shape, given).filter(|_| !value.is_empty());
            let sub = match (then, value.is_empty()) {
                (_, true) => format!("{what}{}", needed(field)),
                (Some(f), false) => format!("{what} · Tab: {}:…{}", f.name, needed(f)),
                (None, false) => format!("{what} · every field is there"),
            };
            let label = if value.is_empty() { format!("{}:…", field.name) } else { format!("{}:{value}", field.name) };
            vec![Choice { label, sub, text: value.to_string(), next: then.map(|f| format!("{}:", f.name)) }]
        }
        None => shape
            .fields
            .iter()
            .filter(|f| !given.iter().any(|g| f.name.eq_ignore_ascii_case(g)))
            .map(|f| Choice { label: format!("{}:…{}", f.name, needed(f)), sub: format!("{} sources have no {name}:", shape.platform), text: value.to_string(), next: None })
            .collect(),
    }
}

/// A source's data as the server takes it, from its fields as typed (`values`: name,
/// text; lists one item per line): only the fields filled in, as their type says.
pub fn data_from(shape: &SourceShape, values: &[(String, String)]) -> Result<Value, String> {
    let mut data = serde_json::Map::new();
    for field in &shape.fields {
        let text = values.iter().find(|(n, _)| *n == field.name).map(|(_, v)| v.trim()).unwrap_or("");
        if text.is_empty() {
            if field.required {
                return Err(format!("{} is needed.", field.name.replace('_', " ")));
            }
            continue;
        }
        let value = match field.kind.as_str() {
            "int" => Value::from(text.parse::<i64>().map_err(|_| format!("{} is a whole number.", field.name.replace('_', " ")))?),
            "strs" => Value::from(text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>()),
            _ => Value::from(text),
        };
        data.insert(field.name.clone(), value);
    }
    Ok(Value::Object(data))
}

/// What a link says, for a `kind` of source ("track", "album", "artist"): the platform
/// and fields to fill in, as the extensions read it (SoundCloud, YouTube, NetEase…); any
/// other web link: a custom source named after its site, with the link. A link on a
/// platform's site that is no source of this kind is none.
pub fn from_link(link: &str, kind: &str) -> Option<(String, Vec<(String, String)>)> {
    let link = link.trim();
    let registry = crate::extensions::current();
    if let Some(found) = registry.match_link(link, kind, false) {
        return Some((found.platform, found.fields));
    }
    let rest = link.strip_prefix("https://").or_else(|| link.strip_prefix("http://"))?;
    if registry.reads_site_of(link) {
        return None;
    }
    let host = rest.split('/').next().unwrap_or(rest);
    let host = host.trim_start_matches("www.").trim_start_matches("m.");
    Some(("custom".to_string(), vec![("label".to_string(), host.to_string()), ("url".to_string(), link.to_string())]))
}

