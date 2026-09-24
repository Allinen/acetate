//! The data shape of each platform's sources, per kind of entity.
//!
//! Source data is supplied by whoever uploads (e.g. migration tooling with its own
//! logic for finding SoundCloud / NetEase / YouTube info). It's validated strictly:
//! unknown fields or wrong types are rejected rather than stored, so it stays queryable.
//!
//! A snapshot holds one value per field: the data as it was at one point in time.
//! Changes over time (a new permalink...) are new snapshots of the same source.
//!
//! To support a new platform, entity kind or field: add/extend an entry in `SHAPES`.
//! That's the only place; the database stores any platform listed here.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

/// What a source belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceOwner {
    Track(Uuid),
    Album(Uuid),
    Artist(Uuid),
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
            SourceOwner::Track(id) | SourceOwner::Album(id) | SourceOwner::Artist(id) => *id,
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            SourceOwner::Track(_) => Kind::Track,
            SourceOwner::Album(_) => Kind::Album,
            SourceOwner::Artist(_) => Kind::Artist,
        }
    }
}

impl Kind {
    fn name(&self) -> &'static str {
        match self {
            Kind::Track => "track",
            Kind::Album => "album",
            Kind::Artist => "artist",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    Int,
    Str,
}

pub struct Field {
    pub name: &'static str,
    pub field_type: FieldType,
    pub required: bool,
}

pub struct Shape {
    pub platform: &'static str,
    pub kind: Kind,
    /// The field holding the platform's id for the owner. Snapshots with the same id
    /// belong to the same source. None if the platform has no id (e.g. custom).
    pub id_field: Option<&'static str>,
    pub fields: &'static [Field],
}

use FieldType::{Int, Str};

/// Shorthands to keep `SHAPES` readable.
const fn required(name: &'static str, field_type: FieldType) -> Field {
    Field {
        name,
        field_type,
        required: true,
    }
}
const fn optional(name: &'static str, field_type: FieldType) -> Field {
    Field {
        name,
        field_type,
        required: false,
    }
}

const CUSTOM_FIELDS: &[Field] = &[
    // Short name of where it's from: "discord", "google drive", "a friend"...
    required("label", Str),
    // Anything else: who sent it, when, which server/channel...
    optional("description", Str),
];

/// Every supported (platform, kind) and its fields.
pub const SHAPES: &[Shape] = &[
    // ---- SoundCloud ----
    Shape {
        platform: "soundcloud",
        kind: Kind::Track,
        id_field: Some("song_id"),
        fields: &[
            required("song_id", Int),
            optional("song_uid", Str),
            optional("song_permalink", Str),
            optional("song_description", Str),
        ],
    },
    Shape {
        platform: "soundcloud",
        kind: Kind::Album,
        id_field: None,
        fields: &[required("album_permalink", Str)],
    },
    Shape {
        platform: "soundcloud",
        kind: Kind::Artist,
        id_field: None,
        fields: &[required("artist_permalink", Str)],
    },
    // ---- NetEase ----
    Shape {
        platform: "netease",
        kind: Kind::Track,
        id_field: Some("song_id"),
        fields: &[required("song_id", Int)],
    },
    Shape {
        platform: "netease",
        kind: Kind::Album,
        id_field: Some("album_id"),
        fields: &[required("album_id", Int)],
    },
    Shape {
        platform: "netease",
        kind: Kind::Artist,
        id_field: Some("artist_id"),
        fields: &[required("artist_id", Int)],
    },
    // ---- YouTube ----
    Shape {
        platform: "youtube",
        kind: Kind::Track,
        id_field: Some("song_id"),
        fields: &[
            required("song_id", Str), // video id
            optional("song_description", Str),
            // The video's release date, verbatim. Independent of the track's own
            // (canonical) release date.
            optional("song_release_date", Str),
        ],
    },
    // ---- Custom: anything without a real platform (Discord, Google Drive...) ----
    Shape {
        platform: "custom",
        kind: Kind::Track,
        id_field: None,
        fields: CUSTOM_FIELDS,
    },
    Shape {
        platform: "custom",
        kind: Kind::Album,
        id_field: None,
        fields: CUSTOM_FIELDS,
    },
    Shape {
        platform: "custom",
        kind: Kind::Artist,
        id_field: None,
        fields: CUSTOM_FIELDS,
    },
];

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

    let Some(shape) = SHAPES
        .iter()
        .find(|s| s.platform == source.platform && s.kind == kind)
    else {
        return err("no data shape defined".to_string());
    };
    let Some(given) = source.data.as_object() else {
        return err("`data` must be a JSON object".to_string());
    };

    for key in given.keys() {
        if !shape.fields.iter().any(|f| f.name == key) {
            let known: Vec<&str> = shape.fields.iter().map(|f| f.name).collect();
            return err(format!("unknown field `{key}`, expected one of {known:?}"));
        }
    }

    for field in shape.fields {
        match given.get(field.name) {
            None if field.required => return err(format!("`{}` is required", field.name)),
            None => {}
            Some(value) => {
                let ok = match field.field_type {
                    Int => value.is_i64(),
                    Str => value.is_string(),
                };
                if !ok {
                    let expected = match field.field_type {
                        Int => "an integer",
                        Str => "a string",
                    };
                    return err(format!("`{}` must be {expected}, got {value}", field.name));
                }
            }
        }
    }

    let external_id = shape.id_field.and_then(|name| match given.get(name)? {
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
        assert!(validate(Kind::Track, new("soundcloud", json!({ "song_permalink": "x" }))).is_err());
        // Not an object.
        assert!(validate(Kind::Track, new("netease", json!([]))).is_err());
        // Unknown platform / kind combination.
        assert!(validate(Kind::Track, new("spotify", json!({}))).is_err());
        assert!(validate(Kind::Album, new("youtube", json!({}))).is_err());
    }

    #[test]
    fn shapes_are_consistent() {
        for (i, a) in SHAPES.iter().enumerate() {
            for b in &SHAPES[i + 1..] {
                assert!(!(a.platform == b.platform && a.kind == b.kind), "{} twice", a.platform);
            }
            if let Some(id) = a.id_field {
                let field = a.fields.iter().find(|f| f.name == id);
                assert!(field.is_some_and(|f| f.required), "{}: id field must be required", a.platform);
            }
        }
    }
}
