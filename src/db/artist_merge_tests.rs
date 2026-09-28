//! Merging artists when credits use aliases ("credited as"): the aliases go along, and
//! every credit ends up on the kept artist under the right name, whatever the two had.
//!
//! The library: see `rls_tests::setup` (artists Both and X, track O credited to Both).

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

use super::rls_tests::{Library, make_track, setup};
use crate::db;
use crate::db::revisions::Source;
use crate::model::{Alias, ArtistData, Credit, TrackMetadata};

/// An artist with these aliases; returns (artist, alias ids in the same order).
async fn artist(lib: &Library, name: &str, aliases: &[&str]) -> anyhow::Result<(Uuid, Vec<Uuid>)> {
    let mut tx = db::begin(&lib.owner, None).await?;
    let id = db::ingest::find_or_create_artist(&mut tx, name).await?.id;
    let data = ArtistData {
        name: name.into(),
        aliases: aliases.iter().map(|a| Alias { id: None, name: a.to_string() }).collect(),
        ..Default::default()
    };
    db::revisions::write_artist(&mut tx, id, None, &data, Source::Edit, "").await?;
    let mut ids = Vec::new();
    for alias in aliases {
        let alias_id: Uuid = sqlx::query_scalar("SELECT id FROM artist_aliases WHERE artist_id = $1 AND alias = $2 AND listed")
            .bind(id)
            .bind(alias)
            .fetch_one(&mut *tx)
            .await?;
        ids.push(alias_id);
    }
    tx.commit().await?;
    Ok((id, ids))
}

/// A track credited to `artist` as `alias` (None: its name). Returns the track.
async fn credited(lib: &Library, title: &str, artist: Uuid, alias: Option<Uuid>) -> anyhow::Result<Uuid> {
    let mut tx = db::begin(&lib.owner, None).await?;
    let (track, _) = make_track(&mut tx, title, &[artist], None).await?;
    let variant: Uuid = sqlx::query_scalar("SELECT primary_variant_id FROM tracks WHERE id = $1").bind(track).fetch_one(&mut *tx).await?;
    let meta = TrackMetadata {
        title: title.into(),
        credits: vec![Credit { artist_id: artist, role: "primary".into(), alias_id: alias }],
        ..Default::default()
    };
    db::revisions::write_internal(&mut tx, track, variant, None, &meta, Source::Edit, "", false).await?;
    tx.commit().await?;
    Ok(track)
}

/// The track's credit now: (artist, the alias's name if any), and its artist text.
async fn credit_of(lib: &Library, track: Uuid) -> anyhow::Result<((Uuid, Option<String>), String)> {
    let credit: (Uuid, Option<String>) = sqlx::query_as(
        "SELECT c.artist_id, al.alias FROM track_credits c LEFT JOIN artist_aliases al ON al.id = c.alias_id WHERE c.track_id = $1",
    )
    .bind(track)
    .fetch_one(&lib.owner)
    .await?;
    let text: String = sqlx::query_scalar("SELECT artist_display FROM tracks WHERE id = $1").bind(track).fetch_one(&lib.owner).await?;
    Ok((credit, text))
}

async fn merge(lib: &Library, from: Uuid, into: Uuid, keep_name: bool) -> anyhow::Result<()> {
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    db::revisions::merge_artist(&mut tx, from, into, keep_name, false, "").await?;
    tx.commit().await?;
    Ok(())
}

#[sqlx::test]
async fn an_alias_named_like_the_kept_artist(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    // "Zed" has an alias "X" (the kept artist's own name), credited on T.
    let (zed, aliases) = artist(&lib, "Zed", &["X"]).await?;
    let t = credited(&lib, "Song T", zed, Some(aliases[0])).await?;
    merge(&lib, zed, lib.x, false).await?;
    // T is X's now, under X's own name (no alias needed: it's the same name).
    assert_eq!(credit_of(&lib, t).await?, ((lib.x, None), "X".to_string()));
    Ok(())
}

#[sqlx::test]
async fn an_alias_both_have(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let (zed, zed_aliases) = artist(&lib, "Zed", &["Zee"]).await?;
    let (ex, _) = artist(&lib, "Ex", &["Zee"]).await?;
    let t = credited(&lib, "Song T", zed, Some(zed_aliases[0])).await?;
    merge(&lib, zed, ex, false).await?;
    // Credited as Ex's own "Zee".
    assert_eq!(credit_of(&lib, t).await?, ((ex, Some("Zee".into())), "Zee".to_string()));
    Ok(())
}

#[sqlx::test]
async fn an_alias_only_the_merged_one_has(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let (zed, zed_aliases) = artist(&lib, "Zed", &["Zee"]).await?;
    let t = credited(&lib, "Song T", zed, Some(zed_aliases[0])).await?;
    merge(&lib, zed, lib.x, false).await?;
    // The alias moved to X: T still shows "Zee".
    assert_eq!(credit_of(&lib, t).await?, ((lib.x, Some("Zee".into())), "Zee".to_string()));
    Ok(())
}

#[sqlx::test]
async fn keeping_the_old_name(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let (zed, _) = artist(&lib, "Zed", &[]).await?;
    let t = credited(&lib, "Song T", zed, None).await?;
    merge(&lib, zed, lib.x, true).await?;
    // Credited to X as "Zed", an alias of X now.
    assert_eq!(credit_of(&lib, t).await?, ((lib.x, Some("Zed".into())), "Zed".to_string()));
    Ok(())
}
