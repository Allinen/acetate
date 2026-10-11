//! Extensions: what Acetate knows about a place music comes from (YouTube, SoundCloud…),
//! as data (`extension.toml`, see docs/admin/extensions.md). The server loads them and
//! serves them as JSON (`GET /api/extensions`); the clients read the same, so a link is
//! understood the same everywhere.
//!
//! * **shapes**: the fields a platform's sources hold, per kind (track, album, artist);
//! * **links**: templates (`https://youtu.be/{song_id}`) that turn a pasted link into a
//!   source's fields, and the source's own page;
//! * **ids**: how a platform's ids look (an 11-character YouTube id), and whether they end
//!   a downloader's file name.

mod link;

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use link::LinkMatch;

/// The kinds of thing a source can be of.
pub const KINDS: [&str; 3] = ["track", "album", "artist"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub extension: Header,
    #[serde(default, rename = "shape")]
    pub shapes: Vec<Shape>,
    #[serde(default, rename = "link")]
    pub links: Vec<Link>,
    #[serde(default)]
    pub id: Option<IdRule>,
    /// The program for what data can't say (not yet read: see the design).
    #[serde(default, skip_serializing)]
    pub fetch: Option<Fetch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    /// The platform name stored with every source.
    pub name: String,
    pub title: String,
    /// The API version it needs.
    pub api: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shape {
    pub kind: String,
    /// The field that says which source it is (snapshots with the same one are one source).
    #[serde(default)]
    pub id: Option<String>,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub name: String,
    /// `int`, `str` or `strs` (a list of strings).
    #[serde(rename = "type")]
    pub kind: FieldType,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Int,
    Str,
    Strs,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Link {
    pub kind: String,
    /// The source's page, with its fields filled in.
    pub open: Option<String>,
    /// Links this one reads, as templates: `{name}` is a part of the link to keep.
    #[serde(rename = "match")]
    pub matches: Vec<String>,
    /// The fields the parts make, when they aren't the fields as they are:
    /// `album_permalinks = "{user}/sets/{set}"`.
    pub set: HashMap<String, String>,
    /// What a part has to look like.
    pub captures: HashMap<String, Capture>,
    /// A link in a file's comment counts (the first of the comment's links that does).
    pub comment: bool,
    /// A custom source with this label, for links to these hosts (`amazon.*`: any ending).
    pub custom: Option<String>,
    pub hosts: Vec<String>,
    /// For `custom`: the link's path has this in it.
    pub path_contains: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Capture {
    /// Not these.
    pub not: Vec<String>,
    pub len: Option<usize>,
    pub class: Option<IdClass>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IdClass {
    /// Digits only.
    Digits,
    /// Letters, digits, `-` and `_`.
    Id,
}

impl Capture {
    fn allows(&self, text: &str) -> bool {
        !text.is_empty()
            && !self.not.iter().any(|n| n == text)
            && self.len.is_none_or(|n| text.chars().count() == n)
            && self.class.is_none_or(|class| text.bytes().all(|b| class.allows(b)))
    }
}

impl IdClass {
    fn allows(self, b: u8) -> bool {
        match self {
            IdClass::Digits => b.is_ascii_digit(),
            IdClass::Id => b.is_ascii_alphanumeric() || b == b'-' || b == b'_',
        }
    }
}

/// How a platform's ids of one kind look, and where they're found.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdRule {
    pub kind: String,
    pub field: String,
    #[serde(default)]
    pub class: Option<IdClass>,
    #[serde(default)]
    pub len: Option<usize>,
    /// The id ends a downloader's file name: `Title [id].ext`.
    #[serde(default)]
    pub filename: bool,
    /// An id on its own is a link to the thing.
    #[serde(default)]
    pub bare: bool,
}

impl IdRule {
    /// Whether `text` looks like one of this platform's ids.
    pub fn is_id(&self, text: &str) -> bool {
        Capture { not: Vec::new(), len: self.len, class: self.class }.allows(text)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fetch {
    pub command: Vec<String>,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub at_once: Option<usize>,
    #[serde(default)]
    pub per_user_per_minute: Option<usize>,
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub env: Vec<String>,
}

/// Every extension that is loaded, in order (a later one with the same name replaced the
/// earlier).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Registry {
    pub extensions: Vec<Manifest>,
}

const BUILTIN: &[(&str, &str)] = &[
    ("soundcloud", include_str!("../../../extensions/soundcloud/extension.toml")),
    ("distrokid", include_str!("../../../extensions/distrokid/extension.toml")),
    ("netease", include_str!("../../../extensions/netease/extension.toml")),
    ("qq", include_str!("../../../extensions/qq/extension.toml")),
    ("kuwo", include_str!("../../../extensions/kuwo/extension.toml")),
    ("youtube", include_str!("../../../extensions/youtube/extension.toml")),
    ("custom", include_str!("../../../extensions/custom/extension.toml")),
    ("stores", include_str!("../../../extensions/stores/extension.toml")),
];

/// The API version this build speaks (extensions needing a newer one are skipped).
pub const API: u32 = parse_number(include_str!("../../../API_VERSION"));

/// (`str::parse` isn't `const`.)
const fn parse_number(text: &str) -> u32 {
    let bytes = text.as_bytes();
    let mut n = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            n = n * 10 + (bytes[i] - b'0') as u32;
        }
        i += 1;
    }
    n
}

impl Manifest {
    /// Reads one `extension.toml`; Err says what is wrong with it.
    pub fn parse(text: &str) -> Result<Manifest, String> {
        let manifest: Manifest = toml::from_str(text).map_err(|e| e.to_string())?;
        manifest.check()?;
        Ok(manifest)
    }

    fn check(&self) -> Result<(), String> {
        let name = &self.extension.name;
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
            return Err(format!("the name “{name}” is lowercase letters, digits and _"));
        }
        for shape in &self.shapes {
            if !KINDS.contains(&shape.kind.as_str()) {
                return Err(format!("“{}” isn't a kind (track, album, artist)", shape.kind));
            }
            if let Some(id) = &shape.id
                && !shape.fields.iter().any(|f| &f.name == id)
            {
                return Err(format!("the {} shape's id “{id}” isn't one of its fields", shape.kind));
            }
        }
        for link in &self.links {
            if !KINDS.contains(&link.kind.as_str()) && !(link.custom.is_some() && link.kind == "any") {
                return Err(format!("a link's kind “{}” isn't track, album or artist", link.kind));
            }
            link::check(self, link)?;
        }
        Ok(())
    }
}

impl Registry {
    /// A registry of these (later ones replace earlier ones of the same name).
    pub fn of(manifests: Vec<Manifest>) -> Registry {
        let mut registry = Registry::default();
        for manifest in manifests {
            registry.add(manifest);
        }
        registry
    }

    pub fn add(&mut self, manifest: Manifest) {
        self.extensions.retain(|e| e.extension.name != manifest.extension.name);
        self.extensions.push(manifest);
    }

    /// The extensions that come with Acetate.
    pub fn builtin() -> &'static Registry {
        static BUILTIN_REGISTRY: OnceLock<Registry> = OnceLock::new();
        BUILTIN_REGISTRY.get_or_init(|| {
            Registry::of(
                BUILTIN
                    .iter()
                    .map(|(name, text)| Manifest::parse(text).unwrap_or_else(|e| panic!("the built-in extension {name}: {e}")))
                    .collect(),
            )
        })
    }

    /// The built-in extensions and then those in the folders `dir/*/extension.toml`
    /// (replacing a built-in of the same name). What can't be loaded is returned with why;
    /// the rest is used.
    pub fn with_folder(dir: &std::path::Path) -> (Registry, Vec<String>) {
        let mut registry = Registry::builtin().clone();
        let mut problems = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            problems.push(format!("{}: can't be read", dir.display()));
            return (registry, problems);
        };
        let mut folders: Vec<_> = entries.flatten().map(|e| e.path()).filter(|p| p.join("extension.toml").is_file()).collect();
        folders.sort();
        for folder in folders {
            let file = folder.join("extension.toml");
            let loaded = std::fs::read_to_string(&file).map_err(|e| e.to_string()).and_then(|text| Manifest::parse(&text));
            match loaded {
                Ok(manifest) if manifest.extension.api > API => {
                    problems.push(format!("{}: needs API {} and this is {API}", file.display(), manifest.extension.api));
                }
                Ok(manifest) => registry.add(manifest),
                Err(why) => problems.push(format!("{}: {why}", file.display())),
            }
        }
        (registry, problems)
    }

    /// Every shape with the platform it is of.
    pub fn shapes(&self) -> impl Iterator<Item = (&str, &Shape)> {
        self.extensions.iter().flat_map(|e| e.shapes.iter().map(move |s| (e.extension.name.as_str(), s)))
    }

    pub fn shape(&self, platform: &str, kind: &str) -> Option<&Shape> {
        self.shapes().find(|(p, s)| *p == platform && s.kind == kind).map(|(_, s)| s)
    }

    pub fn id_rule(&self, platform: &str) -> Option<&IdRule> {
        self.extensions.iter().find(|e| e.extension.name == platform)?.id.as_ref()
    }

    /// The platforms whose downloader file names end with their id, with the rule.
    pub fn filename_ids(&self) -> impl Iterator<Item = (&str, &IdRule)> {
        self.extensions.iter().filter_map(|e| Some((e.extension.name.as_str(), e.id.as_ref().filter(|i| i.filename)?)))
    }

    /// The source a link is, for a `kind` of source (`comment`: only links that count in a
    /// file's comment). Links are matched by the first extension that reads them.
    pub fn match_link(&self, link: &str, kind: &str, comment: bool) -> Option<LinkMatch> {
        let parts = link::Parts::of(link)?;
        self.extensions.iter().find_map(|e| {
            e.links.iter().filter(|l| (l.kind == kind || l.kind == "any") && (!comment || l.comment)).find_map(|l| link::read(e, l, &parts, link))
        })
    }

    /// Whether an extension has links on this link's site, whatever kind of thing they
    /// are of: a link there that no template reads isn't a source of any kind.
    pub fn reads_site_of(&self, link: &str) -> bool {
        let Some(parts) = link::Parts::of(link) else { return false };
        self.extensions.iter().flat_map(|e| &e.links).any(|l| l.custom.is_none() && l.matches.iter().any(|t| link::template_host(t) == parts.host()))
    }

    /// The video, song… an `id` on its own stands for (`bare` ids), as a link match.
    pub fn match_bare_id(&self, text: &str, kind: &str) -> Option<LinkMatch> {
        let text = text.trim();
        self.extensions.iter().find_map(|e| {
            let rule = e.id.as_ref().filter(|r| r.bare && r.kind == kind && r.is_id(text))?;
            Some(LinkMatch { platform: e.extension.name.clone(), kind: kind.to_string(), fields: vec![(rule.field.clone(), text.to_string())] })
        })
    }

    /// The page of a source, from its data.
    pub fn open_url(&self, platform: &str, kind: &str, data: &Value) -> Option<String> {
        let extension = self.extensions.iter().find(|e| e.extension.name == platform)?;
        extension.links.iter().filter(|l| l.kind == kind).find_map(|l| link::fill(l.open.as_deref()?, |name| link::value_text(data.get(name)?)))
    }

    /// The data a link's fields make (`song_permalinks`, a list, from one permalink), or why not.
    pub fn data_from_fields(&self, platform: &str, kind: &str, fields: &[(String, String)]) -> Result<Value, String> {
        let shape = self.shape(platform, kind).ok_or_else(|| format!("{platform} has no {kind} sources"))?;
        let mut data = serde_json::Map::new();
        for (name, text) in fields {
            let field = shape.fields.iter().find(|f| &f.name == name).ok_or_else(|| format!("{platform} sources have no “{name}”"))?;
            let value = match field.kind {
                FieldType::Int => Value::from(text.parse::<i64>().map_err(|_| format!("{name} is a whole number"))?),
                FieldType::Strs => Value::from(vec![text.clone()]),
                FieldType::Str => Value::from(text.clone()),
            };
            data.insert(name.clone(), value);
        }
        Ok(Value::Object(data))
    }
}

#[cfg(test)]
mod tests;
