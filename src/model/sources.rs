//! The data shape of each platform's sources, per kind of entity.
//!
//! Source data is supplied by whoever uploads (e.g. migration tooling with its own
//! logic for finding SoundCloud / NetEase / YouTube info). It's validated strictly:
//! unknown fields or wrong types are rejected rather than stored, so it stays queryable.
//!
//! A snapshot holds one value per field: the data as it was at one point in time.
//! Changes over time (a new permalink...) are new snapshots of the same source.
//!
//! The shapes are the extensions' (`extensions/*/extension.toml`, see `crate::extensions`):
//! to support a new platform, entity kind or field, add or extend one. The database stores
//! any platform they list.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use acetate_extensions::FieldType;

/// What a source belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceOwner {
    Track(Uuid),
    Album(Uuid),
    Artist(Uuid),
    /// A grouping (a collective: the platform accounts it is). Its sources have the
    /// shapes of an artist's: an account is one.
    Grouping(Uuid),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Track,
    Album,
    Artist,
}

impl SourceOwner {
    pub fn id(&self) -> Uuid {
        match self {
            SourceOwner::Track(id) | SourceOwner::Album(id) | SourceOwner::Artist(id) | SourceOwner::Grouping(id) => *id,
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            SourceOwner::Track(_) => Kind::Track,
            SourceOwner::Album(_) => Kind::Album,
            SourceOwner::Artist(_) | SourceOwner::Grouping(_) => Kind::Artist,
        }
    }
}

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Track => "track",
            Kind::Album => "album",
            Kind::Artist => "artist",
        }
    }
}

/// A source snapshot as sent by a client (upload form, CLI, API).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewSource {
    pub platform: String,
    /// Add the snapshot to this existing source. Only useful for platforms without an
    /// id field (e.g. a SoundCloud artist whose permalink changed); others are matched by id.
    pub source_id: Option<Uuid>,
    /// When the data was observed on the platform. Defaults to now.
    pub captured_at: Option<DateTime<Utc>>,
    pub data: serde_json::Value,
}

/// A `NewSource` whose data passed validation.
#[derive(Debug, Clone)]
pub struct ValidSource {
    pub platform: String,
    pub source_id: Option<Uuid>,
    pub captured_at: Option<DateTime<Utc>>,
    /// The value of the shape's id field, as text.
    pub external_id: Option<String>,
    /// Only the fields that were given.
    pub data: serde_json::Value,
}

/// Source data that doesn't fit. The message is meant for the client.
#[derive(Debug)]
pub struct InvalidSource(pub String);

impl std::fmt::Display for InvalidSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for InvalidSource {}

/// Checks `source.data` against the shape for its platform and the owner's kind.
pub fn validate(kind: Kind, source: NewSource) -> Result<ValidSource, InvalidSource> {
    let err = |msg: String| {
        Err(InvalidSource(format!(
            "{} {}: {msg}",
            source.platform,
            kind.name()
        )))
    };

    let Some(shape) = crate::extensions::registry().shape(&source.platform, kind.name()) else {
        return err("no data shape defined".to_string());
    };
    let Some(given) = source.data.as_object() else {
        return err("`data` must be a JSON object".to_string());
    };

    for key in given.keys() {
        if !shape.fields.iter().any(|f| &f.name == key) {
            let known: Vec<&str> = shape.fields.iter().map(|f| f.name.as_str()).collect();
            return err(format!("unknown field `{key}`, expected one of {known:?}"));
        }
    }

    for field in &shape.fields {
        match given.get(&field.name) {
            None if field.required => return err(format!("`{}` is required", field.name)),
            None => {}
            Some(value) => {
                let ok = match field.kind {
                    FieldType::Int => value.is_i64(),
                    FieldType::Str => value.is_string(),
                    FieldType::Strs => value.as_array().is_some_and(|items| items.iter().all(|i| i.is_string())),
                };
                if !ok {
                    let expected = match field.kind {
                        FieldType::Int => "an integer",
                        FieldType::Str => "a string",
                        FieldType::Strs => "a list of strings",
                    };
                    return err(format!("`{}` must be {expected}, got {value}", field.name));
                }
            }
        }
    }

    let external_id = shape.id.as_ref().and_then(|name| match given.get(name)? {
        serde_json::Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    });

    Ok(ValidSource {
        platform: source.platform,
        source_id: source.source_id,
        captured_at: source.captured_at,
        external_id,
        data: source.data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn new(platform: &str, data: serde_json::Value) -> NewSource {
        NewSource {
            platform: platform.to_string(),
            source_id: None,
            captured_at: None,
            data,
        }
    }

    #[test]
    fn soundcloud_track() {
        let v = validate(
            Kind::Track,
            new(
                "soundcloud",
                json!({ "song_id": 555, "song_permalink": "https://soundcloud.com/a/b" }),
            ),
        )
        .unwrap();
        assert_eq!(v.external_id.as_deref(), Some("555"));
        assert_eq!(
            v.data,
            json!({ "song_id": 555, "song_permalink": "https://soundcloud.com/a/b" })
        );
    }

    #[test]
    fn youtube_ids_are_strings() {
        let v = validate(Kind::Track, new("youtube", json!({ "song_id": "dQw4w9WgXcQ" }))).unwrap();
        assert_eq!(v.external_id.as_deref(), Some("dQw4w9WgXcQ"));
        assert!(validate(Kind::Track, new("youtube", json!({ "song_id": 123 }))).is_err());
    }

    #[test]
    fn custom_has_no_id() {
        let v = validate(
            Kind::Track,
            new("custom", json!({ "label": "discord", "description": "sent by X" })),
        )
        .unwrap();
        assert_eq!(v.external_id, None);
        assert!(validate(Kind::Track, new("custom", json!({ "description": "x" }))).is_err());
    }

    #[test]
    fn rejects_bad_data() {
        // Lists aren't single values.
        assert!(validate(Kind::Track, new("netease", json!({ "song_id": [1] }))).is_err());
        // Wrong type.
        assert!(validate(Kind::Track, new("netease", json!({ "song_id": "1" }))).is_err());
        // Unknown field.
        assert!(validate(Kind::Track, new("netease", json!({ "song_id": 1, "x": 2 }))).is_err());
        // Missing required field.
        assert!(validate(Kind::Track, new("netease", json!({ "song_index": 1 }))).is_err());
        assert!(validate(Kind::Track, new("distrokid", json!({ "index": 1 }))).is_err());
        // A list where one value is expected, and the other way round.
        assert!(validate(Kind::Track, new("soundcloud", json!({ "song_permalinks": "a/b" }))).is_err());
        assert!(validate(Kind::Track, new("soundcloud", json!({ "song_permalinks": ["a/b", 1] }))).is_err());
        // Not an object.
        assert!(validate(Kind::Track, new("netease", json!([]))).is_err());
        // Unknown platform / kind combination.
        assert!(validate(Kind::Track, new("spotify", json!({}))).is_err());
        assert!(validate(Kind::Album, new("youtube", json!({}))).is_err());
    }

    #[test]
    fn soundcloud_song_by_permalink_only() {
        let v = validate(Kind::Track, new("soundcloud", json!({ "song_permalinks": ["1kori/spinback"] }))).unwrap();
        assert_eq!(v.external_id, None);
    }

    #[test]
    fn distrokid() {
        let track = validate(
            Kind::Track,
            new("distrokid", json!({ "album_uuid": "C47AFBE2-678D-4595-926C7586E304CA81", "index": 1 })),
        )
        .unwrap();
        assert_eq!(track.external_id, None);
        let album = validate(Kind::Album, new("distrokid", json!({ "album_uuid": "C47AFBE2-678D-4595-926C7586E304CA81" }))).unwrap();
        assert_eq!(album.external_id.as_deref(), Some("C47AFBE2-678D-4595-926C7586E304CA81"));
        let artist = validate(Kind::Artist, new("distrokid", json!({ "artist_id": 1001517, "artist_uuid": "x" }))).unwrap();
        assert_eq!(artist.external_id.as_deref(), Some("1001517"));
    }
}

/// The Python ingestion library (`clients/python`) has these shapes as typed classes, in
/// a file made from the built-in extensions' shapes. This checks it's up to date; to make it again after changing
/// the shapes: `ACETATE_WRITE_PYTHON_SHAPES=1 cargo test python_shapes`.
#[cfg(test)]
mod python_shapes {
    use super::*;

    const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/clients/python/acetate_ingest/_shapes.py");

    /// How the platform is written in class names (`NetEaseTrack`).
    fn class_prefix(platform: &str) -> String {
        match platform {
            "soundcloud" => "SoundCloud".into(),
            "distrokid" => "DistroKid".into(),
            "netease" => "NetEase".into(),
            "qq" => "QQ".into(),
            "youtube" => "YouTube".into(),
            other => {
                let mut chars = other.chars();
                chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
            }
        }
    }

    fn render() -> String {
        let mut out = String::from(
            "# Made from the server's source shapes (`extensions/*/extension.toml`): don't edit.\n\
             # `cargo test` checks it's up to date; `ACETATE_WRITE_PYTHON_SHAPES=1 cargo test python_shapes`\n\
             # makes it again.\n\n\
             from dataclasses import dataclass\n\
             from typing import ClassVar\n\n\
             from ._source import Field, Shape, ShapedSource\n\n\
             SHAPES: dict[tuple[str, str], Shape] = {\n",
        );
        for (platform, shape) in acetate_extensions::Registry::builtin().shapes() {
            let id = shape.id.as_ref().map_or("None".to_string(), |f| format!("\"{f}\""));
            out += &format!("    (\"{}\", \"{}\"): Shape(id_field={id}, fields={{\n", platform, shape.kind);
            for field in &shape.fields {
                let kind = match field.kind {
                    FieldType::Int => "int",
                    FieldType::Str => "str",
                    FieldType::Strs => "strs",
                };
                let required = if field.required { "True" } else { "False" };
                out += &format!("        \"{}\": Field(\"{kind}\", required={required}),\n", field.name);
            }
            out += "    }),\n";
        }
        out += "}\n";
        let mut names = Vec::new();
        for (platform, shape) in acetate_extensions::Registry::builtin().shapes() {
            let kind = shape.kind.as_str();
            let name = format!("{}{}{}", class_prefix(platform), kind[..1].to_uppercase(), &kind[1..]);
            let id = shape.id.as_ref().map_or(String::from("none: sources with the same data are the same"), |f| format!("`{f}`"));
            out += &format!(
                "\n\n@dataclass(frozen=True, kw_only=True)\nclass {name}(ShapedSource):\n    \"\"\"A {kind}'s source on {}. Its id: {id}.\"\"\"\n\n    platform: ClassVar[str] = \"{}\"\n    kind: ClassVar[str] = \"{kind}\"\n",
                platform, platform
            );
            for field in &shape.fields {
                assert!(!["source_id", "captured_at"].contains(&field.name.as_str()), "{name}: `{}` is taken", field.name);
                let python_type = match field.kind {
                    FieldType::Int => "int",
                    FieldType::Str => "str",
                    FieldType::Strs => "list[str]",
                };
                if field.required {
                    out += &format!("    {}: {python_type}\n", field.name);
                } else {
                    out += &format!("    {}: {python_type} | None = None\n", field.name);
                }
            }
            names.push(name);
        }
        out += "\n\n__all__ = [\n";
        for name in names {
            out += &format!("    \"{name}\",\n");
        }
        out += "]\n";
        out
    }

    #[test]
    fn python_shapes_are_up_to_date() {
        let made = render();
        if std::env::var_os("ACETATE_WRITE_PYTHON_SHAPES").is_some() {
            std::fs::write(PATH, &made).unwrap();
            return;
        }
        let kept = std::fs::read_to_string(PATH).unwrap_or_default();
        assert!(
            kept == made,
            "{PATH} doesn't match the extensions' shapes: make it again with `ACETATE_WRITE_PYTHON_SHAPES=1 cargo test python_shapes`"
        );
    }
}
