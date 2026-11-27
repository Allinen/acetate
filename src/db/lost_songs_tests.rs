//! Lost songs (`db::lost_songs`, migration 0072). The library: see `rls_tests::setup` (V
//! and H on album A, H restricted to friends, which bob is in and carol isn't; O on no album).

use serde_json::json;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

use super::rls_tests::setup;
use crate::db;
use crate::db::extras::{About, Lost, LostStatus};
use crate::db::lookup::PlatformId;
use crate::db::lost_songs::{Known, NewLostSong};
use crate::model::sources::{self, Kind, NewSource, SourceOwner};

fn sc(id: i64) -> NewSource {
    NewSource { platform: "soundcloud".into(), source_id: None, captured_at: None, data: json!({ "song_id": id, "song_permalink": format!("a/song-{id}") }) }
}

fn lost(status: LostStatus) -> Lost {
    Lost { status, artists_shown: " X ft. Both ".into(), release_date: Some("2019-05".into()), duration_ms: Some(150_000) }
}

fn song(name: &str, artist: Uuid, sources: Vec<NewSource>) -> NewLostSong {
    NewLostSong { name: name.into(), artist_ids: vec![artist], lost: lost(LostStatus::Lost), description: String::new(), sources }
}

async fn flags(tx: &mut sqlx::PgConnection, track: Uuid) -> anyhow::Result<Vec<(Uuid, serde_json::Value, bool)>> {
    Ok(sqlx::query_as::<_, (Uuid, serde_json::Value, bool)>(
        "SELECT id, related, resolved_at IS NOT NULL FROM review_flags WHERE track_id = $1 AND kind = 'lost_song_found' ORDER BY id",
    )
    .bind(track)
    .fetch_all(tx)
    .await?)
}

#[sqlx::test]
async fn a_lost_song_has_its_facts_and_sources_and_is_looked_up(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    sqlx::query("INSERT INTO sources (track_id, platform, external_id) VALUES ($1, 'soundcloud', '1')").bind(lib.v).execute(&mut *tx).await?;

    let id = db::lost_songs::create(&mut tx, &song(" Gone ", lib.x, vec![sc(99)]), "").await?.expect("made");
    let extra = db::extras::get(&mut tx, id).await?.unwrap();
    assert_eq!((extra.name.as_str(), extra.kind.as_str()), ("Gone", "lost_song"));
    let facts = extra.lost.unwrap();
    assert_eq!((facts.status, facts.artists_shown.as_str(), facts.release_date.as_deref()), (LostStatus::Lost, "X ft. Both", Some("2019-05")));
    assert_eq!(extra.sources.iter().map(|s| s.external_id.clone()).collect::<Vec<_>>(), [Some("99".to_string())]);
    assert_eq!(extra.links.iter().map(|l| l.id).collect::<Vec<_>>(), [lib.x]);

    let asked = vec![PlatformId { platform: "soundcloud".into(), external_id: "99".into() }];
    assert_eq!(db::lookup::sources(&mut tx, &asked).await?[0].owners, json!([{ "kind": "lost_song", "id": id }]));

    match db::lost_songs::create(&mut tx, &song("Gone again", lib.x, vec![sc(99)]), "").await? {
        Err(Known::AlreadyLost { extra_id, .. }) => assert_eq!(extra_id, id),
        other => panic!("{other:?}"),
    }
    match db::lost_songs::create(&mut tx, &song("Not gone", lib.both, vec![sc(1)]), "").await? {
        Err(Known::AlreadyHeld { track_id, .. }) => assert_eq!(track_id, lib.v),
        other => panic!("{other:?}"),
    }
    assert!(db::lost_songs::create(&mut tx, &NewLostSong { artist_ids: vec![], ..song("Nobody's", lib.x, vec![]) }, "").await.is_err(), "needs an artist");
    let mut bad = song("Bad date", lib.x, vec![]);
    bad.lost.release_date = Some("2019-13".into());
    assert!(db::lost_songs::create(&mut tx, &bad, "").await.is_err());

    assert_eq!(db::lost_songs::list(&mut tx, Some(lib.x), None, 100).await?.len(), 1);
    assert_eq!(db::lost_songs::list(&mut tx, Some(lib.x), Some(LostStatus::Proposed), 100).await?.len(), 0);
    assert_eq!(db::lost_songs::list(&mut tx, Some(lib.both), None, 100).await?.len(), 0);

    // Only a lost song has facts and sources, and one with sources stays one.
    let about = About::default();
    assert!(db::extras::create(&mut tx, "Stems", "stems", &about, &[], None, Some(&lost(LostStatus::Lost)), "").await.is_err());
    assert!(db::extras::create(&mut tx, "Lost", "lost_song", &about, &[], None, None, "").await.is_err());
    let (stems, _) = db::extras::create(&mut tx, "Stems", "stems", &about, &[], None, None, "").await?;
    let valid = sources::validate(Kind::Track, sc(5))?;
    assert!(!db::sources::owner_exists(&mut tx, SourceOwner::Extra(stems)).await?, "an extra that isn't a lost song has no sources");
    assert!(db::extras::edit(&mut tx, id, "Gone", "clip", &about, None, "").await.is_err(), "it has sources");
    assert!(db::extras::edit(&mut tx, id, "Gone", "lost_song", &about, Some(&lost(LostStatus::Snippet)), "").await?);
    assert_eq!(db::extras::get(&mut tx, id).await?.unwrap().lost.unwrap().status, LostStatus::Snippet);
    db::sources::add_snapshot(&mut tx, SourceOwner::Extra(id), &valid).await?;
    Ok(())
}

#[sqlx::test]
async fn a_track_with_a_lost_songs_source_is_flagged_and_found(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let gone = db::lost_songs::create(&mut tx, &song("Gone", lib.x, vec![sc(99)]), "").await?.unwrap();
    let other = db::lost_songs::create(&mut tx, &song("Other", lib.x, vec![sc(77)]), "").await?.unwrap();
    tx.commit().await?;

    // The track gets the platform id (ingest, an edit): flagged when that commits.
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    db::sources::add_snapshot(&mut tx, SourceOwner::Track(lib.o), &sources::validate(Kind::Track, sc(99))?).await?;
    tx.commit().await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let raised = flags(&mut tx, lib.o).await?;
    assert_eq!(raised.len(), 1);
    assert_eq!(raised[0].1, json!({ "extra_id": gone, "platform": "soundcloud", "external_id": "99" }));

    // The other way round: a lost song given a track's platform id.
    db::sources::add_snapshot(&mut tx, SourceOwner::Track(lib.v), &sources::validate(Kind::Track, sc(1))?).await?;
    tx.commit().await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    db::sources::add_snapshot(&mut tx, SourceOwner::Extra(other), &sources::validate(Kind::Track, sc(1))?).await?;
    tx.commit().await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let on_v = flags(&mut tx, lib.v).await?;
    assert_eq!(on_v.len(), 1);
    // Not it: resolved, and not raised again when the source comes and goes.
    assert!(db::review::resolve(&mut tx, on_v[0].0, "not this song").await?);
    let source = db::sources::list_for(&mut tx, other).await?.into_iter().find(|s| s.external_id.as_deref() == Some("1")).unwrap();
    assert!(db::sources::trash(&mut tx, source.id).await?);
    db::sources::add_snapshot(&mut tx, SourceOwner::Extra(other), &sources::validate(Kind::Track, sc(1))?).await?;
    tx.commit().await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    assert_eq!(flags(&mut tx, lib.v).await?.len(), 1, "not raised twice");

    // Found: the track gets the lost song's sources, and the lost song is taken off.
    let found = db::lost_songs::found(&mut tx, raised[0].0).await?.unwrap();
    assert_eq!((found.track_id, found.extra_id, found.sources_added), (lib.o, gone, 0), "O had song 99 already");
    tx.commit().await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    assert!(flags(&mut tx, lib.o).await?[0].2, "settled");
    assert!(db::extras::get(&mut tx, gone).await?.unwrap().hidden);
    let asked = vec![PlatformId { platform: "soundcloud".into(), external_id: "99".into() }];
    assert_eq!(db::lookup::sources(&mut tx, &asked).await?[0].owners, json!([{ "kind": "track", "id": lib.o }]), "not the lost song taken off");
    assert!(db::lost_songs::found(&mut tx, raised[0].0).await?.is_none(), "once");
    Ok(())
}

#[sqlx::test]
async fn found_copies_the_sources_the_track_lacks(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let gone = db::lost_songs::create(&mut tx, &song("Gone", lib.x, vec![sc(99), sc(98)]), "").await?.unwrap();
    db::sources::add_snapshot(&mut tx, SourceOwner::Track(lib.o), &sources::validate(Kind::Track, sc(98))?).await?;
    tx.commit().await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let flag = flags(&mut tx, lib.o).await?[0].0;
    let found = db::lost_songs::found(&mut tx, flag).await?.unwrap();
    assert_eq!((found.extra_id, found.sources_added), (gone, 1));
    let mut ids: Vec<_> = db::sources::list_for(&mut tx, lib.o).await?.into_iter().filter_map(|s| s.external_id).collect();
    ids.sort();
    assert_eq!(ids, ["98", "99"]);
    tx.commit().await?;
    // Copying 99 to the track (in the same transaction as taking it off) raised nothing.
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    assert_eq!(flags(&mut tx, lib.o).await?.len(), 1);
    Ok(())
}

#[sqlx::test]
async fn lost_songs_merge_with_their_sources(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let kept = db::lost_songs::create(&mut tx, &song("Gone", lib.x, vec![sc(99)]), "").await?.unwrap();
    let dupe = db::lost_songs::create(&mut tx, &song("Gone (2)", lib.x, vec![sc(98)]), "").await?.unwrap();
    let (stems, _) = db::extras::create(&mut tx, "Stems", "stems", &About::default(), &[], None, None, "").await?;
    assert!(db::extras::merge(&mut tx, kept, &[stems], "").await.is_err(), "not with an extra that isn't a lost song");

    db::extras::merge(&mut tx, kept, &[dupe], "").await?;
    let mut ids: Vec<_> = db::extras::get(&mut tx, kept).await?.unwrap().sources.into_iter().filter_map(|s| s.external_id).collect();
    ids.sort();
    assert_eq!(ids, ["98", "99"]);
    assert!(db::extras::unmerge(&mut tx, dupe, "").await?);
    let back: Vec<_> = db::extras::get(&mut tx, dupe).await?.unwrap().sources.into_iter().filter_map(|s| s.external_id).collect();
    assert_eq!(back, ["98"]);
    Ok(())
}

#[sqlx::test]
async fn a_restricted_lost_songs_sources_are_hidden_with_it(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let id = db::lost_songs::create(&mut tx, &song("Secret", lib.x, vec![sc(99)]), "").await?.unwrap();
    db::extras::set_access(&mut tx, id, &db::extras::Access { restricted: true, groups: vec![lib.friends] }, "").await?;
    tx.commit().await?;
    let asked = vec![PlatformId { platform: "soundcloud".into(), external_id: "99".into() }];
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert_eq!(db::lookup::sources(&mut tx, &asked).await?[0].owners, json!([]));
    assert!(db::lost_songs::list(&mut tx, None, None, 100).await?.is_empty());
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    assert_eq!(db::lookup::sources(&mut tx, &asked).await?[0].owners, json!([{ "kind": "lost_song", "id": id }]));
    Ok(())
}
