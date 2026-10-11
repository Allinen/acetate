//! Metadata from elsewhere, for the track editor's "import metadata from": a platform's
//! track read by its extension's program (YouTube's with `yt-dlp`, optional on the server
//! like ffmpeg: where it's missing, this says so, and pasting its JSON still works), or a
//! pasted JSON the extensions recognize. Both answer with the same `Metadata`.

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use serde_json::{Value, json};

use acetate_extensions::Metadata;

use super::{AppError, AppState};
use crate::auth::CurrentUser;
use crate::extensions::{fetch, registry};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    /// A link to the track (or its id on its own, where the platform has those).
    input: String,
}

/// `POST /api/extensions/{name}/metadata`, JSON `{"input": "…"}`: the track at this link on
/// the platform, read by its extension's program. 400 for what isn't one of the platform's
/// links; 503 where the program's tool isn't installed (yt-dlp); 422 when it can't read the
/// track (with why) or takes too long; 429 past the extension's limits (YouTube: 10 a
/// minute per person, 3 at once).
pub async fn metadata(State(state): State<AppState>, user: CurrentUser, Path(name): Path<String>, Json(body): Json<Input>) -> Result<Json<Metadata>, AppError> {
    Ok(Json(by_input(&state, &user, &name, &body.input).await?))
}

/// The track a link (or id) is on `platform`, read by the extension's program.
pub async fn by_input(state: &AppState, user: &CurrentUser, platform: &str, input: &str) -> Result<Metadata, AppError> {
    let registry = registry();
    let title = registry.extension(platform).map_or(platform, |e| e.extension.title.as_str());
    let found = registry
        .match_bare_id(input, "track")
        .or_else(|| registry.match_link(input, "track", false))
        .filter(|m| m.platform == platform)
        .ok_or_else(|| AppError::BadRequest(format!("not a link or id of a track on {title}")))?;
    let fields: serde_json::Map<String, Value> = found.fields.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    let remember = format!("{:?}", found.fields);
    let request = json!({ "op": "metadata", "input": input.trim(), "match": fields });
    Ok(fetch::metadata(state, user, platform, &request, Some(&remember)).await?)
}

/// `POST /api/extensions/import`, JSON: a pasted JSON object (a yt-dlp info JSON, a
/// SoundCloud track's). The extension that recognizes it (`[import]`) reads it. 400 when
/// none does.
pub async fn import(State(state): State<AppState>, user: CurrentUser, Json(pasted): Json<Value>) -> Result<Json<Metadata>, AppError> {
    let registry = registry();
    let Some(extension) = registry.detect_import(&pasted) else {
        let known: Vec<&str> = registry.extensions.iter().filter(|e| e.import.is_some()).map(|e| e.extension.title.as_str()).collect();
        return Err(AppError::BadRequest(format!("not the JSON of a platform Acetate reads ({})", known.join(", "))));
    };
    let request = json!({ "op": "parse", "input": pasted });
    Ok(Json(fetch::metadata(&state, &user, &extension.extension.name, &request, None).await?))
}

#[cfg(test)]
mod tests {
    use crate::extensions::registry;

    #[test]
    fn pasted_json_is_told_apart_by_the_extensions() {
        let r = registry();
        let name = |json: serde_json::Value| r.detect_import(&json).map(|e| e.extension.name.clone());
        assert_eq!(name(serde_json::json!({ "id": "x", "extractor_key": "Youtube" })).as_deref(), Some("youtube"));
        assert_eq!(name(serde_json::json!({ "webpage_url": "https://www.youtube.com/watch?v=x" })).as_deref(), Some("youtube"));
        assert_eq!(name(serde_json::json!({ "id": 1, "waveform_url": "x" })).as_deref(), Some("soundcloud"));
        assert_eq!(name(serde_json::json!({ "id": 1, "permalink_url": "https://soundcloud.com/a/b" })).as_deref(), Some("soundcloud"));
        assert_eq!(name(serde_json::json!({ "id": 1 })), None);
        assert_eq!(name(serde_json::json!({ "id": "1", "extractor_key": "Soundcloud" })), None);
    }
}
