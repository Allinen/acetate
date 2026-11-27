//! Review flags: things a person should look at, noticed by ingest (possible duplicates,
//! ambiguous artist names) or left by a merge (tracks whose credits it moved). Nothing is
//! changed automatically; resolving a flag just records that someone looked (and what
//! they did).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::revisions::EditRejected;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::model::Credit;

/// How similar a title must be (pg_trgm `similarity`, 0 to 1) to count as a possible
/// duplicate. "Northbound" vs "Northbound (Live)" is about 0.6.
const DUPLICATE_SIMILARITY: f32 = 0.5;

/// How far apart two tracks' lengths may be (in milliseconds) and still be the same song.
/// Encoders and rips differ by a second or so; songs that differ by more are different
/// songs (an edit, the extended version), whatever their titles say.
const DUPLICATE_MAX_LENGTH_DIFFERENCE_MS: i64 = 3_000;

/// Words that make a title another version of a song: "Song" and "Song (Instrumental)"
/// aren't duplicates, however similar the titles. The library's own (the `version_word`
/// list of the vocabulary). Each maps to a mark, so spellings of the same thing agree
/// ("Remastered" and "Remaster"). Several words are matched in a row; the ones marked
/// `substring` (for languages written without spaces between words) are found anywhere in
/// the title.
pub struct VersionWords {
    phrases: Vec<(Vec<String>, String)>,
    substrings: Vec<(String, String)>,
}

impl VersionWords {
    pub async fn load(conn: &mut PgConnection) -> sqlx::Result<VersionWords> {
        let rows = sqlx::query!(
            r#"SELECT lower(word) AS "word!", data ->> 'mark' AS "mark!", COALESCE((data ->> 'substring')::boolean, false) AS "substring!"
               FROM vocabulary WHERE list = 'version_word' ORDER BY word"#
        )
        .fetch_all(conn)
        .await?;
        let mut words = VersionWords { phrases: Vec::new(), substrings: Vec::new() };
        for row in rows {
            if row.substring {
                words.substrings.push((row.word, row.mark));
            } else {
                let phrase: Vec<String> = row.word.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect();
                if !phrase.is_empty() {
                    words.phrases.push((phrase, row.mark));
                }
            }
        }
        // Longer phrases first: "speed up" before a "speed" of its own.
        words.phrases.sort_by_key(|(phrase, _)| std::cmp::Reverse(phrase.len()));
        Ok(words)
    }

    /// What makes a title a particular version: its version words and its numbers ("Song
    /// 2", "Part II"), each as often as it appears. Two tracks are possible duplicates
    /// only if theirs are the same: "Song" and "Song (Sped Up)" aren't, nor "Song" and
    /// "Song 2", nor "Live Forever" and "Live Forever (Live)"; "Song (Remix)" and "Song -
    /// remix" are.
    pub fn marks(&self, title: &str) -> Vec<String> {
        let title = title.to_lowercase();
        let words: Vec<&str> = title.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
        let mut marks = Vec::new();
        let mut i = 0;
        while i < words.len() {
            if let Some((phrase, mark)) = self.phrases.iter().find(|(phrase, _)| words[i..].len() >= phrase.len() && phrase.iter().zip(&words[i..]).all(|(a, b)| a == b)) {
                marks.push(mark.clone());
                i += phrase.len();
                continue;
            }
            let word = words[i];
            let roman = ["ii", "iii", "iv", "vi", "vii", "viii", "ix"].iter().position(|r| *r == word);
            if let Some(n) = roman {
                marks.push([2, 3, 4, 6, 7, 8, 9][n].to_string());
            } else if word.bytes().all(|b| b.is_ascii_digit()) {
                let n = word.trim_start_matches('0');
                marks.push(if n.is_empty() { "0" } else { n }.to_string());
            }
            i += 1;
        }
        for (text, mark) in &self.substrings {
            marks.extend(std::iter::repeat_n(mark.clone(), title.matches(text.as_str()).count()));
        }
        marks.sort();
        marks
    }
}

/// Flags a track whose artist name from the tags matched several artists; `chosen` (the
/// oldest) was credited.
pub async fn flag_ambiguous_artist(
    conn: &mut PgConnection,
    track_id: Uuid,
    name: &str,
    chosen: Uuid,
    artists: &[Uuid],
) -> sqlx::Result<()> {
    let related = json!({ "name": name, "chosen": chosen, "artists": artists });
    sqlx::query!(
        "INSERT INTO review_flags (kind, track_id, related) VALUES ('ambiguous_artist', $1, $2)",
        track_id,
        related
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// A hand edit changed how a track credits the candidates of its open ambiguous-artist
/// flags (`before` → `after`: its shown version's credits): someone decided who the name
/// is, so each such flag is settled, saying who's credited now. Edits that leave the
/// candidates' credits as they were (a producer added) leave the flags open.
pub async fn settle_ambiguous_by_edit(conn: &mut PgConnection, track_id: Uuid, before: &[Credit], after: &[Credit]) -> sqlx::Result<()> {
    let flags = sqlx::query!(
        "SELECT id, related FROM review_flags WHERE track_id = $1 AND kind = 'ambiguous_artist' AND resolved_at IS NULL",
        track_id
    )
    .fetch_all(&mut *conn)
    .await?;
    for flag in flags {
        let candidates: Vec<Uuid> = flag.related["artists"].as_array().into_iter().flatten().filter_map(|a| a.as_str()?.parse().ok()).collect();
        let of = |credits: &[Credit]| -> Vec<Credit> { credits.iter().filter(|c| candidates.contains(&c.artist_id)).cloned().collect() };
        let now = of(after);
        if of(before) == now {
            continue;
        }
        let ids: Vec<Uuid> = now.iter().map(|c| c.artist_id).collect();
        let names = sqlx::query_scalar!("SELECT name FROM artists WHERE id = ANY($1) ORDER BY array_position($1, id)", &ids)
            .fetch_all(&mut *conn)
            .await?;
        let resolution = if names.is_empty() {
            "credits edited: none of its artists credited".to_string()
        } else {
            format!("credits edited: \"{}\" credited", names.join("\", \""))
        };
        resolve(conn, flag.id, &resolution).await?;
    }
    Ok(())
}

/// Puts a track on the Review page to be fixed later, with the importer's note (e.g.
/// "imported quickly: check title and credits").
pub async fn flag_fix_later(conn: &mut PgConnection, track_id: Uuid, note: &str) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO review_flags (kind, track_id, related) VALUES ('fix_later', $1, $2)",
        track_id,
        json!({ "note": note })
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Flags a track whose credits a merge moved from `from` to `into`, so someone can check
/// the result (e.g. "A feat. B" merged into A: B may need crediting again).
pub async fn flag_merged_artist(
    conn: &mut PgConnection,
    track_id: Uuid,
    from: Uuid,
    from_name: &str,
    into: Uuid,
) -> sqlx::Result<()> {
    let related = json!({ "from": from, "from_name": from_name, "into": into, "artists": [from, into] });
    sqlx::query!(
        "INSERT INTO review_flags (kind, track_id, related) VALUES ('merged_artist', $1, $2)",
        track_id,
        related
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Flags a new track that has the same platform song id as other tracks (e.g. the same
/// song of a platform downloaded twice, as different files). A possible duplicate, with the reason.
pub async fn flag_same_song(
    conn: &mut PgConnection,
    track_id: Uuid,
    tracks: &[Uuid],
    reason: &str,
) -> sqlx::Result<()> {
    let related = json!({ "tracks": tracks, "reason": reason });
    sqlx::query!(
        "INSERT INTO review_flags (kind, track_id, related) VALUES ('possible_duplicate', $1, $2)",
        track_id,
        related
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Flags a new track if tracks by the same artists have a similar title and about the same
/// length (a track or file without a known length isn't ruled out), and are the same
/// version (`VersionWords::marks`). `except`: tracks already flagged for another reason (see
/// `flag_same_song`).
///
/// Starts from the artists' tracks rather than from all similar titles: an artist has at
/// most thousands of tracks, while a short title like "Intro" is similar to far more.
pub async fn flag_possible_duplicates(
    conn: &mut PgConnection,
    track_id: Uuid,
    title: &str,
    artist_ids: &[Uuid],
    except: &[Uuid],
) -> sqlx::Result<()> {
    let candidates = sqlx::query!(
        r#"
        WITH new AS (
            SELECT f.duration_ms FROM tracks t JOIN files f ON f.id = t.file_id WHERE t.id = $1
        )
        SELECT t.id, t.title FROM tracks t
        JOIN files f ON f.id = t.file_id
        WHERE t.id IN (SELECT track_id FROM track_credits WHERE artist_id = ANY($3))
          AND t.id <> $1
          AND t.id <> ALL($5)
          AND t.merged_into IS NULL
          AND similarity(t.title, $2) >= $4
          AND (f.duration_ms IS NULL OR (SELECT duration_ms FROM new) IS NULL
               OR abs(f.duration_ms - (SELECT duration_ms FROM new)) <= $6)
        ORDER BY similarity(t.title, $2) DESC, t.id
        LIMIT 100
        "#,
        track_id,
        title,
        artist_ids,
        DUPLICATE_SIMILARITY,
        except,
        DUPLICATE_MAX_LENGTH_DIFFERENCE_MS,
    )
    .fetch_all(&mut *conn)
    .await?;
    let words = VersionWords::load(&mut *conn).await?;
    let marks = words.marks(title);
    let similar: Vec<Uuid> = candidates.into_iter().filter(|t| words.marks(&t.title) == marks).map(|t| t.id).take(10).collect();
    if similar.is_empty() {
        return Ok(());
    }
    let related = json!({ "tracks": similar });
    sqlx::query!(
        "INSERT INTO review_flags (kind, track_id, related) VALUES ('possible_duplicate', $1, $2)",
        track_id,
        related
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct Flag {
    pub id: Uuid,
    pub kind: String,
    pub track_id: Uuid,
    pub track_title: String,
    pub track_artist: String,
    /// As stored (see the migration), minus the `tracks` / `artists` id lists: those are
    /// expanded below instead, which leaves out the ones the user can't see.
    pub related: serde_json::Value,
    /// For possible duplicates: the similar tracks, `[{id, title, artist_display}]`. One
    /// merged since shows as the track it was merged into (not twice, and not if that's
    /// the flagged track itself).
    pub related_tracks: serde_json::Value,
    /// For ambiguous artists: the candidates; for merges: both artists.
    /// `[{id, name, disambiguation}]`.
    pub related_artists: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by: Option<String>,
    pub resolution: String,
    /// Where the track's file was imported from, if known (`files.imported_from`): for
    /// going through flags by folder.
    pub imported_from: Option<String>,
}

/// Which flags `list` gives.
#[derive(Debug, Default)]
pub struct Filter<'a> {
    /// Resolved flags instead of open ones.
    pub resolved: bool,
    /// Only this kind.
    pub kind: Option<&'a str>,
    /// All but this kind.
    pub exclude: Option<&'a str>,
    /// Only automatic kinds (true: what tracks are missing), or none of them (false).
    pub automatic: Option<bool>,
    /// Only this track's.
    pub track: Option<Uuid>,
}

/// Flags, newest first, a page at a time (`before` = last id seen).
pub async fn list(conn: &mut PgConnection, filter: &Filter<'_>, before: Option<Uuid>, limit: i64) -> sqlx::Result<Vec<Flag>> {
    sqlx::query_as!(
        Flag,
        r#"
        SELECT f.id, f.kind, f.track_id, t.title AS track_title, t.artist_display AS track_artist,
               f.related - 'tracks' - 'artists' AS "related!",
               -- A named track merged since is the one it went into (each once; not the
               -- flagged track itself: that pair is settled).
               COALESCE((
                   SELECT jsonb_agg(jsonb_build_object(
                       'id', rt.id, 'title', rt.title, 'artist_display', rt.artist_display) ORDER BY rt.id)
                   FROM tracks rt
                   WHERE rt.id IN (SELECT final_track((jsonb_array_elements_text(f.related -> 'tracks'))::uuid))
                     AND rt.id <> f.track_id
               ), '[]') AS "related_tracks!",
               COALESCE((
                   SELECT jsonb_agg(jsonb_build_object(
                       'id', a.id, 'name', a.name, 'disambiguation', a.disambiguation))
                   FROM artists a
                   WHERE a.id IN (SELECT (jsonb_array_elements_text(f.related -> 'artists'))::uuid)
               ), '[]') AS "related_artists!",
               f.created_at, f.resolved_at, u.username AS "resolved_by?", f.resolution,
               fi.imported_from
        FROM review_flags f
        JOIN vocabulary k ON k.list = 'review_kind' AND k.word = f.kind
        JOIN tracks t ON t.id = f.track_id
        LEFT JOIN files fi ON fi.id = t.file_id
        LEFT JOIN users u ON u.id = f.resolved_by
        WHERE (f.resolved_at IS NOT NULL) = $1
          AND f.id < COALESCE($2, 'ffffffff-ffff-ffff-ffff-ffffffffffff'::uuid)
          AND ($4::text IS NULL OR f.kind = $4)
          AND ($5::text IS NULL OR f.kind <> $5)
          AND ($6::bool IS NULL OR (k.data ->> 'automatic')::boolean = $6)
          AND ($7::uuid IS NULL OR f.track_id = $7)
        ORDER BY f.id DESC
        LIMIT $3
        "#,
        filter.resolved,
        before,
        limit,
        filter.kind,
        filter.exclude,
        filter.automatic,
        filter.track,
    )
    .fetch_all(conn)
    .await
}

/// Marks a flag as resolved by the transaction's user. False if there's no such open flag.
pub async fn resolve(conn: &mut PgConnection, id: Uuid, resolution: &str) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        r#"
        UPDATE review_flags
        SET resolved_at = now(), resolved_by = current_app_user(), resolution = $2
        WHERE id = $1 AND resolved_at IS NULL
        "#,
        id,
        resolution
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Marks many flags resolved, with one `resolution` (the Review page's bulk tools). Only
/// open ones the user can see. Returns how many were.
pub async fn resolve_many(conn: &mut PgConnection, ids: &[Uuid], resolution: &str) -> sqlx::Result<u64> {
    let result = sqlx::query!(
        "UPDATE review_flags SET resolved_at = now(), resolved_by = current_app_user(), resolution = $2
         WHERE id = ANY($1) AND resolved_at IS NULL",
        ids,
        resolution
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// Settles the "possible duplicate" question between two tracks only, e.g. after they
/// were made to share a file: they're not the same track, and that's decided. Flags about
/// other tracks stay as they are.
///
/// A flag on one of them naming only the other is resolved with `resolution`. A flag that
/// names more tracks stays open for those: the other one is taken off its list and noted
/// under `related.settled` (with the resolution and when). Returns the flags changed.
pub async fn settle_duplicate_pair(
    conn: &mut PgConnection,
    a: Uuid,
    b: Uuid,
    resolution: &str,
) -> sqlx::Result<Vec<Uuid>> {
    settle_pair(conn, a, b, resolution, true).await
}

/// `settle_duplicate_pair`, with or without the flags raised for the same platform song
/// (`flag_same_song`: the ones with a `reason`).
async fn settle_pair(
    conn: &mut PgConnection,
    a: Uuid,
    b: Uuid,
    resolution: &str,
    same_song_too: bool,
) -> sqlx::Result<Vec<Uuid>> {
    let mut changed = Vec::new();
    // Both directions: a flag on `a` naming `b`, and one on `b` naming `a`.
    for (flagged, other) in [(a, b), (b, a)] {
        let ids = sqlx::query_scalar!(
            r#"
            UPDATE review_flags SET
                -- The last track on the list: the whole flag is settled (the list stays, as
                -- a record of what it was about).
                resolved_at = CASE WHEN jsonb_array_length(related -> 'tracks') = 1 THEN now() END,
                resolved_by = CASE WHEN jsonb_array_length(related -> 'tracks') = 1 THEN current_app_user() END,
                resolution = CASE WHEN jsonb_array_length(related -> 'tracks') = 1 THEN $3 ELSE resolution END,
                -- Otherwise: off the list, onto `settled`.
                related = CASE WHEN jsonb_array_length(related -> 'tracks') = 1 THEN related ELSE
                    jsonb_set(related, '{tracks}', (related -> 'tracks') - $2::text)
                    || jsonb_build_object('settled', COALESCE(related -> 'settled', '[]'::jsonb)
                        || jsonb_build_array(jsonb_build_object('track', $2::uuid, 'resolution', $3::text, 'at', now())))
                END
            WHERE track_id = $1 AND kind = 'possible_duplicate' AND resolved_at IS NULL
              AND (related -> 'tracks') ? $2::text
              AND ($4 OR NOT related ? 'reason')
            RETURNING id
            "#,
            flagged,
            other.to_string(),
            resolution,
            same_song_too
        )
        .fetch_all(&mut *conn)
        .await?;
        changed.extend(ids);
    }
    Ok(changed)
}

/// A kind of flag, as `GET /api/vocab` lists them.
#[derive(Debug, Serialize)]
pub struct Kind {
    pub kind: String,
    /// For people ("No cover").
    pub name: String,
    /// Raised and settled by the server itself, from what the track has (see migration
    /// 0030): "no title", "no cover"… The others are raised by ingest and merges.
    pub automatic: bool,
    /// For automatic kinds: whether the server checks for it.
    pub checked: bool,
    /// Made by people, from the app (see `make_kind`).
    pub custom: bool,
}

/// Every kind of flag, in the order to show them.
pub async fn kinds(conn: &mut PgConnection) -> sqlx::Result<Vec<Kind>> {
    sqlx::query_as!(
        Kind,
        r#"SELECT word AS "kind!", label AS "name!", (data ->> 'automatic')::boolean AS "automatic!", (data ->> 'checked')::boolean AS "checked!", NOT locked AS "custom!"
           FROM vocabulary WHERE list = 'review_kind' ORDER BY position, word"#
    )
    .fetch_all(conn)
    .await
}

/// A kind of flag made by a person (see migration 0056), named `name`: returns its key.
/// Refused if a kind has that name, or the key it makes, already.
pub async fn make_kind(conn: &mut PgConnection, name: &str) -> anyhow::Result<String> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let key: String = name.to_lowercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join("_");
    if key.is_empty() {
        return Err(EditRejected::Invalid("a kind of flag needs a name with letters or digits".into()).into());
    }
    let taken = sqlx::query_scalar!(r#"SELECT label AS "name!" FROM vocabulary WHERE list = 'review_kind' AND (word = $1 OR lower(label) = lower($2))"#, key, name)
        .fetch_optional(&mut *conn)
        .await?;
    if let Some(taken) = taken {
        return Err(EditRejected::Invalid(format!("there's a kind of flag named “{taken}” already")).into());
    }
    Ok(sqlx::query_scalar!(r#"SELECT make_review_kind($1) AS "kind!""#, name).fetch_one(conn).await?)
}

/// Flags a track by hand with `kind` ("Fix later" or one made by people), unless it has
/// an open flag of that kind already. Returns whether it was flagged.
pub async fn flag_by_hand(conn: &mut PgConnection, track_id: Uuid, kind: &str) -> anyhow::Result<bool> {
    let found = sqlx::query!(r#"SELECT label AS "name!", NOT locked AS "custom!" FROM vocabulary WHERE list = 'review_kind' AND word = $1"#, kind).fetch_optional(&mut *conn).await?;
    match found {
        None => return Err(EditRejected::NotFound("kind of flag").into()),
        Some(k) if !k.custom && kind != "fix_later" => {
            return Err(EditRejected::Invalid(format!("“{}” flags are raised by the server, not by hand", k.name)).into());
        }
        Some(_) => {}
    }
    let made = sqlx::query!(
        r#"INSERT INTO review_flags (kind, track_id)
           SELECT $1, $2 WHERE NOT EXISTS (SELECT 1 FROM review_flags WHERE track_id = $2 AND kind = $1 AND resolved_at IS NULL)"#,
        kind,
        track_id
    )
    .execute(conn)
    .await?;
    Ok(made.rows_affected() > 0)
}

/// What a tool proposes for a track (a `proposal` flag's `related`, as sent): at least one
/// of `patch`, `new_variant` and `sources`. Adopting it is an ordinary track edit made by
/// a person on the Review page; nothing here changes the track.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    /// Picked by the proposer: a track has at most one open proposal per key, so proposing
    /// the same thing again is a no-op (e.g. "release_date:soundcloud:123").
    pub key: String,
    /// Who proposed it: the tool, and the model if any ("enrich/qwen3.8-27b").
    pub agent: String,
    /// The proposer's own id for the run, to find its proposals again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    /// The revision `patch` was made against. If the version has a newer one by the time
    /// it's adopted, the edit is refused as stale and a person merges it by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_revision_id: Option<Uuid>,
    /// Metadata fields to change in that version (`TrackMetadata`'s: title, release_date,
    /// genres, comment, credits), as they're to be.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<serde_json::Map<String, serde_json::Value>>,
    /// A whole new version instead, e.g. everything one platform says about the track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_variant: Option<ProposedVariant>,
    /// Sources to add to the track, as `POST /api/tracks/edit` takes them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<ProposedSource>,
    /// Why: where each thing was found.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedVariant {
    pub label: String,
    pub data: crate::model::TrackMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedSource {
    pub platform: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The proposer's record of the check this came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finding_id: Option<String>,
    /// The words on the page that say it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
}

/// The metadata fields a proposal's `patch` may change.
const PATCHABLE: &[&str] = &["title", "release_date", "genres", "comment", "credits"];

/// Flags `track_id` with a proposal, unless it has an open one with the same key. Checked
/// first: the patch applies to its base revision (which is the track's), the new version
/// and the sources are well formed. Stored with `variant_id` (the base revision's) and
/// `fields` (what it changes) filled in, for the Review page. Returns whether it was added.
pub async fn propose(conn: &mut PgConnection, track_id: Uuid, proposal: &Proposal) -> anyhow::Result<bool> {
    let invalid = |msg: String| -> anyhow::Error { EditRejected::Invalid(format!("proposal: {msg}")).into() };
    if proposal.key.trim().is_empty() || proposal.key.len() > 200 {
        return Err(invalid("`key` is 1–200 characters".into()));
    }
    if proposal.agent.trim().is_empty() {
        return Err(invalid("`agent` names who proposed it".into()));
    }
    if proposal.patch.is_none() && proposal.new_variant.is_none() && proposal.sources.is_empty() {
        return Err(invalid("it proposes nothing: give a `patch`, a `new_variant` or `sources`".into()));
    }
    let mut fields: Vec<String> = Vec::new();
    let mut variant_id = None;
    if let Some(patch) = &proposal.patch {
        if let Some(bad) = patch.keys().find(|k| !PATCHABLE.contains(&k.as_str())) {
            return Err(invalid(format!("`patch` can't change `{bad}` (only {PATCHABLE:?})")));
        }
        if patch.is_empty() {
            return Err(invalid("`patch` is empty".into()));
        }
        let base = proposal.base_revision_id.ok_or_else(|| invalid("a `patch` needs the `base_revision_id` it was made against".into()))?;
        let row = sqlx::query!("SELECT variant_id, data FROM track_revisions WHERE id = $1 AND track_id = $2 AND kind = 'internal'", base, track_id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(|| invalid("`base_revision_id` isn't one of this track's revisions".into()))?;
        let mut data = row.data;
        let object = data.as_object_mut().ok_or_else(|| anyhow::anyhow!("revision {base} isn't a JSON object"))?;
        for (key, value) in patch {
            object.insert(key.clone(), value.clone());
        }
        serde_json::from_value::<crate::model::TrackMetadata>(data).map_err(|e| invalid(format!("`patch` doesn't make valid metadata: {e}")))?;
        variant_id = Some(row.variant_id);
        fields.extend(patch.keys().cloned());
    }
    if let Some(variant) = &proposal.new_variant {
        if variant.label.trim().is_empty() {
            return Err(invalid("`new_variant` needs a `label`".into()));
        }
        fields.push("new_variant".into());
    }
    for source in &proposal.sources {
        let new = crate::model::sources::NewSource { platform: source.platform.clone(), source_id: None, captured_at: None, data: source.data.clone() };
        crate::model::sources::validate(crate::model::sources::Kind::Track, new).map_err(|e| invalid(e.to_string()))?;
    }
    if !proposal.sources.is_empty() {
        fields.push("sources".into());
    }
    let mut related = serde_json::to_value(proposal)?;
    related["fields"] = json!(fields);
    if let Some(variant_id) = variant_id {
        related["variant_id"] = json!(variant_id);
    }
    let made = sqlx::query!(
        "INSERT INTO review_flags (kind, track_id, related) VALUES ('proposal', $1, $2)
         ON CONFLICT (track_id, (related ->> 'key')) WHERE kind = 'proposal' AND resolved_at IS NULL DO NOTHING",
        track_id,
        related
    )
    .execute(conn)
    .await?;
    Ok(made.rows_affected() > 0)
}

/// Settles open flags by their ids, if they're the track's (a track editor settling one
/// proposal of several). Returns how many.
pub async fn settle_ids(conn: &mut PgConnection, track_id: Uuid, ids: &[Uuid], resolution: &str) -> sqlx::Result<u64> {
    let result = sqlx::query!(
        "UPDATE review_flags SET resolved_at = now(), resolved_by = current_app_user(), resolution = $3
         WHERE track_id = $1 AND id = ANY($2) AND resolved_at IS NULL",
        track_id,
        ids,
        resolution
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// Settles a track's open flags of `kind` (any kind), with `resolution`. Returns how many.
pub async fn settle_kind(conn: &mut PgConnection, track_id: Uuid, kind: &str, resolution: &str) -> sqlx::Result<u64> {
    let result = sqlx::query!(
        "UPDATE review_flags SET resolved_at = now(), resolved_by = current_app_user(), resolution = $3
         WHERE track_id = $1 AND kind = $2 AND resolved_at IS NULL",
        track_id,
        kind,
        resolution
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// Turns an automatic check on or off (the owner only: `acetate review-checks`), then
/// checks every track again. Off: its open flags are settled as "not checked anymore"
/// first (not "fixed"). On: every track missing it is flagged, even ones someone had
/// marked fine before (turning a check on means "look at all of them again").
/// Returns how many flags are open of that kind afterwards.
pub async fn set_checked(conn: &mut PgConnection, kind: &str, checked: bool) -> anyhow::Result<i64> {
    let automatic = sqlx::query_scalar!(r#"SELECT (data ->> 'automatic')::boolean AS "automatic!" FROM vocabulary WHERE list = 'review_kind' AND word = $1"#, kind)
        .fetch_optional(&mut *conn)
        .await?;
    match automatic {
        None => anyhow::bail!("there's no kind of flag named {kind:?}"),
        Some(false) => anyhow::bail!("{kind:?} isn't an automatic check (ingest and merges raise it)"),
        Some(true) => {}
    }
    sqlx::query!("UPDATE vocabulary SET data = jsonb_set(data, '{checked}', to_jsonb($2::boolean)) WHERE list = 'review_kind' AND word = $1", kind, checked)
        .execute(&mut *conn)
        .await?;
    if !checked {
        sqlx::query!(
            r#"
            UPDATE review_flags SET resolved_at = now(), resolved_by = current_app_user(),
                   resolution = 'not checked anymore'
            WHERE kind = $1 AND resolved_at IS NULL
            "#,
            kind
        )
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query_scalar!(r#"SELECT count(check_track(id)) AS "checked!" FROM tracks"#).fetch_one(&mut *conn).await?;
    let open = sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM review_flags WHERE kind = $1 AND resolved_at IS NULL"#, kind)
        .fetch_one(&mut *conn)
        .await?;
    Ok(open)
}

#[cfg(test)]
mod tests {
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

    use super::VersionWords;

    #[sqlx::test]
    async fn versions_of_a_song_are_told_apart(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
        let pool = PgPoolOptions::new().connect_with(connect).await?;
        let words = VersionWords::load(&mut *pool.acquire().await?).await?;
        let same = |a: &str, b: &str| words.marks(a) == words.marks(b);
        assert!(same("Northbound", "northbound"));
        assert!(same("Northbound (feat. Someone)", "Northbound"));
        assert!(same("Northbound (Remix)", "Northbound - remix"));
        assert!(same("Northbound (Remastered 2011)", "Northbound - 2011 Remaster"));
        assert!(!same("Northbound (Remastered)", "Northbound [2011 Remaster]"), "the year is a number too");
        assert!(same("Part II", "Part 2"));
        assert!(same("Track 07", "Track 7"));
        for other in [
            "Northbound (Instrumental)",
            "Northbound (Inst.)",
            "Northbound (Off Vocal)",
            "Northbound (Sped Up)",
            "Northbound - sped up + reverb",
            "Northbound (Slowed Down)",
            "Northbound (Remastered)",
            "Northbound 2",
            "Northbound Pt. 3",
            "Northbound (Live)",
            "Northbound (Extended Mix)",
            "Northbound (伴奏)",
        ] {
            assert!(!same("Northbound", other), "{other}");
        }
        assert!(!same("Live Forever", "Live Forever (Live)"));
        assert!(!same("Northbound (Sped Up)", "Northbound (Slowed)"));
        assert!(same("Northbound (Sped Up)", "Northbound - SPED UP"));
        Ok(())
    }
}
