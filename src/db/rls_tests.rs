//! Row-level security, end to end. Each test gets a fresh database with all migrations
//! (`#[sqlx::test]` creates it, and drops it afterwards), sets up a small library as the
//! owner (the system), then acts as different users through a pool that connects like the
//! server does (`app_pool_options`), calling the same `db::` functions the API calls.
//!
//! The library:
//! * track V (public) and track H (restricted to the group `friends`), both on album A,
//!   and V is a remix of H;
//! * artist Both, credited on V and H; artist X, credited only on H;
//! * track O (public), on no album;
//! * users alice (admin), bob (user + friends + sharers, who may share), carol (user).
//!
//! Needs Postgres running (`DATABASE_URL`), like building does. `setup` is also used by
//! `trash_tests`.

use serde_json::json;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

use crate::db;
use crate::db::revisions::Source;
use crate::model::sources::{self, Kind, NewSource, SourceOwner};
use crate::model::{AlbumData, AlbumMembership, ArtistData, Credit, Relation, TrackFacts, TrackMetadata};

pub(super) struct Library {
    pub(super) owner: PgPool,
    pub(super) app: PgPool,
    pub(super) alice: Uuid,
    pub(super) bob: Uuid,
    pub(super) carol: Uuid,
    pub(super) friends: Uuid,
    pub(super) v: Uuid,
    pub(super) h: Uuid,
    pub(super) o: Uuid,
    pub(super) h_file: Uuid,
    pub(super) both: Uuid,
    pub(super) x: Uuid,
    pub(super) album: Uuid,
}

pub(super) async fn setup(connect: PgConnectOptions) -> anyhow::Result<Library> {
    let owner = PgPoolOptions::new().connect_with(connect.clone()).await?;
    let app = db::app_pool_options().connect_with(connect).await?;

    let mut tx = db::begin(&owner, None).await?;
    let friends = db::groups::create(&mut tx, "friends", "", &[]).await?;
    db::groups::create(&mut tx, "sharers", "", &["share".to_string()]).await?;
    let alice = add_user(&mut tx, "alice", &["admin"]).await?;
    let bob = add_user(&mut tx, "bob", &["user", "friends", "sharers"]).await?;
    let carol = add_user(&mut tx, "carol", &["user"]).await?;

    let both = db::ingest::find_or_create_artist(&mut tx, "Both").await?.id;
    let x = db::ingest::find_or_create_artist(&mut tx, "X").await?.id;
    let album = db::revisions::create_album(
        &mut tx,
        &AlbumData {
            title: "Album A".into(),
            ..Default::default()
        },
        Source::Import,
        "",
    )
    .await?;

    let (v, _) = make_track(&mut tx, "Song V", &[both], Some(album)).await?;
    let (h, h_file) = make_track(&mut tx, "Song H", &[x, both], Some(album)).await?;
    let (o, _) = make_track(&mut tx, "Other", &[both], None).await?;

    // V is a remix of H.
    let facts = TrackFacts {
        albums: vec![membership(album)],
        relations: vec![Relation {
            kind: "remix_of".into(),
            track_id: h,
        }],
        ..Default::default()
    };
    db::revisions::write_external(&mut tx, v, None, &facts, Source::Edit, "").await?;

    db::access::set(&mut tx, h, true, &[friends], "test").await?;
    tx.commit().await?;

    Ok(Library {
        owner,
        app,
        alice,
        bob,
        carol,
        friends,
        v,
        h,
        o,
        h_file,
        both,
        x,
        album,
    })
}

async fn add_user(conn: &mut sqlx::PgConnection, name: &str, groups: &[&str]) -> anyhow::Result<Uuid> {
    let id = db::users::create_user(conn, name, "not a real hash").await?;
    let groups: Vec<String> = groups.iter().map(|g| g.to_string()).collect();
    db::users::set_groups(conn, id, &groups).await?.map_err(anyhow::Error::msg)?;
    Ok(id)
}

fn membership(album_id: Uuid) -> AlbumMembership {
    AlbumMembership {
        album_id,
        disc_number: None,
        track_number: None,
    }
}

/// A track with a (fake) file, like ingest makes one. Returns (track id, file id).
pub(super) async fn make_track(
    conn: &mut sqlx::PgConnection,
    title: &str,
    artists: &[Uuid],
    album: Option<Uuid>,
) -> anyhow::Result<(Uuid, Uuid)> {
    let file_id = Uuid::now_v7();
    let meta = TrackMetadata {
        title: title.into(),
        credits: artists
            .iter()
            .map(|id| Credit {
                artist_id: *id,
                role: "primary".into(),
                alias_id: None,
            })
            .collect(),
        ..Default::default()
    };
    let facts = TrackFacts {
        albums: album.into_iter().map(membership).collect(),
        ..Default::default()
    };
    let track_id = db::revisions::create_track(conn, file_id, &meta, &facts, "File tags", "test").await?;
    let origin = sources::validate(
        Kind::Track,
        NewSource {
            platform: "custom".into(),
            source_id: None,
            captured_at: None,
            data: json!({ "label": "test" }),
        },
    )?;
    let snapshot = db::sources::add_snapshot(conn, SourceOwner::Track(track_id), &origin).await?;
    db::ingest::insert_file(
        conn,
        &db::ingest::NewFile {
            id: file_id,
            origin_snapshot_id: snapshot.snapshot_id,
            sha256: &crate::auth::token_hash(title),
            original_filename: title,
            uploaded_filename: title,
            size_bytes: 1,
            container: None,
            codec: None,
            bitrate: None,
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_ms: None,
            embedded_tags: &json!({}),
        },
    )
    .await?;
    Ok((track_id, file_id))
}

/// Fails if any of `ids` appears anywhere in `value` (serialized as JSON).
fn assert_mentions_none(what: &str, value: &impl serde::Serialize, ids: &[Uuid]) {
    let text = serde_json::to_string(value).unwrap();
    for id in ids {
        assert!(!text.contains(&id.to_string()), "{what} mentions hidden {id}: {text}");
    }
}

#[sqlx::test]
async fn server_connections_are_rls_bound(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let role: String = sqlx::query_scalar("SELECT current_user::text").fetch_one(&lib.app).await?;
    assert_eq!(role, db::APP_ROLE);
    db::check_row_level_security(&lib.owner).await?;
    Ok(())
}

#[sqlx::test]
async fn restricted_track_is_hidden_everywhere(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let hidden = [lib.h, lib.h_file, lib.x];
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;

    let list = db::tracks::list_tracks(&mut tx, None, 100).await?;
    let mut ids: Vec<Uuid> = list.iter().map(|t| t.id).collect();
    ids.sort();
    let mut v_and_o = vec![lib.v, lib.o];
    v_and_o.sort();
    assert_eq!(ids, v_and_o);
    assert_mentions_none("track list", &list, &hidden);
    assert!(db::tracks::search_tracks(&mut tx, "Song H", None, 100).await?.is_empty());
    assert!(!db::catalog::search_tracks(&mut tx, "Song H", 20).await?.iter().any(|t| t.id == lib.h));
    assert!(db::tracks::get_track(&mut tx, lib.h, true).await?.is_none());

    let files: i64 = sqlx::query_scalar("SELECT count(*) FROM files WHERE id = $1")
        .bind(lib.h_file)
        .fetch_one(&mut *tx)
        .await?;
    assert_eq!(files, 0, "H's file");

    // X is only credited on H: it doesn't exist as far as carol can tell.
    assert!(db::catalog::get_artist(&mut tx, lib.x, true).await?.is_none());
    assert!(db::catalog::search_artists(&mut tx, "X", 20).await?.iter().all(|a| a.id != lib.x));

    // Things that include V and H show V only.
    let album = db::catalog::get_album(&mut tx, lib.album, true).await?.unwrap();
    assert_eq!(album.tracks.len(), 1);
    assert_mentions_none("album", &album, &hidden);
    let both = db::catalog::artist_tracks(&mut tx, lib.both, None, 100).await?;
    assert_eq!(both.len(), 2, "V and O");
    let v = db::tracks::get_track(&mut tx, lib.v, true).await?.unwrap();
    assert!(v.relations.is_empty(), "the relation with H");
    assert_mentions_none("V's details and history", &v, &hidden);
    let flags = db::review::list(&mut tx, false, None, 100).await?;
    assert_mentions_none("review flags", &flags, &hidden);
    drop(tx);

    // Writes can't reach it either.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let updated = sqlx::query("UPDATE tracks SET title = 'mine now' WHERE id = $1")
        .bind(lib.h)
        .execute(&mut *tx)
        .await?;
    assert_eq!(updated.rows_affected(), 0);
    drop(tx);

    // bob is in friends, alice is an admin.
    for (who, id) in [("bob", lib.bob), ("alice", lib.alice)] {
        let mut tx = db::begin(&lib.app, Some(id)).await?;
        assert!(db::tracks::get_track(&mut tx, lib.h, true).await?.is_some(), "{who} sees H");
        assert!(db::catalog::get_artist(&mut tx, lib.x, false).await?.is_some(), "{who} sees X");
        let v = db::tracks::get_track(&mut tx, lib.v, false).await?.unwrap();
        assert_eq!(v.relations.len(), 1, "{who} sees V's relation with H");
    }
    Ok(())
}

/// Whether V is still related to H (as seen by the system).
async fn relation_exists(lib: &Library) -> anyhow::Result<bool> {
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM track_relations WHERE from_track_id = $1 AND to_track_id = $2",
    )
    .bind(lib.v)
    .bind(lib.h)
    .fetch_one(&lib.owner)
    .await?;
    Ok(n == 1)
}

/// Row-level security doesn't look inside JSON: revision data is filtered separately.
#[sqlx::test]
async fn edits_keep_what_the_editor_cant_see(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    // carol edits V's facts: she sees no relations, and sends none.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let v = db::tracks::get_track(&mut tx, lib.v, true).await?.unwrap();
    let facts = TrackFacts {
        status: "unreleased".into(),
        albums: vec![membership(lib.album)],
        relations: Vec::new(),
    };
    db::revisions::write_external(&mut tx, lib.v, v.external_revision_id, &facts, Source::Edit, "")
        .await?;
    tx.commit().await?;
    assert!(relation_exists(&lib).await?, "the relation with H survives carol's edit");
    let latest: serde_json::Value = sqlx::query_scalar(
        "SELECT data FROM track_revisions WHERE track_id = $1 AND kind = 'external' ORDER BY id DESC LIMIT 1",
    )
    .bind(lib.v)
    .fetch_one(&lib.owner)
    .await?;
    assert_eq!(latest["status"], "unreleased");
    assert!(latest.to_string().contains(&lib.h.to_string()), "and V's revision still records it");

    // carol restores V's first revision (from before the relation): only the visible part
    // goes back.
    let first: Uuid = sqlx::query_scalar(
        "SELECT id FROM track_revisions WHERE track_id = $1 AND kind = 'external' ORDER BY id LIMIT 1",
    )
    .bind(lib.v)
    .fetch_one(&lib.owner)
    .await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    db::revisions::restore_track_revision(&mut tx, first, "").await?;
    tx.commit().await?;
    assert!(relation_exists(&lib).await?, "the relation with H survives carol's restore");

    // bob can see H, so his edit does remove it.
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let v = db::tracks::get_track(&mut tx, lib.v, false).await?.unwrap();
    db::revisions::write_external(&mut tx, lib.v, v.external_revision_id, &facts, Source::Edit, "")
        .await?;
    tx.commit().await?;
    assert!(!relation_exists(&lib).await?, "bob removed it");
    Ok(())
}

async fn artist_text(lib: &Library, track: Uuid) -> anyhow::Result<String> {
    Ok(sqlx::query_scalar("SELECT artist_display FROM tracks WHERE id = $1")
        .bind(track)
        .fetch_one(&lib.owner)
        .await?)
}

#[sqlx::test]
async fn cascades_reach_hidden_tracks(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    // Renaming Both (credited on V, O and H) updates H's artist text too.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let both = db::catalog::get_artist(&mut tx, lib.both, false).await?.unwrap();
    let data = ArtistData {
        name: "Both Renamed".into(),
        ..Default::default()
    };
    db::revisions::write_artist(&mut tx, lib.both, both.revision_id, &data, Source::Edit, "").await?;
    tx.commit().await?;
    assert_eq!(artist_text(&lib, lib.h).await?, "X, Both Renamed");

    // Merging Both into a new artist moves H's credit too, but only reports what carol sees.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let target = db::revisions::create_artist(
        &mut tx,
        &ArtistData {
            name: "Target".into(),
            ..Default::default()
        },
        Source::Edit,
        "",
    )
    .await?;
    let merged = db::revisions::merge_artist(&mut tx, lib.both, target, false, true, "").await?;
    tx.commit().await?;
    assert_eq!(merged.variants, 2, "V and O, not H");
    assert_eq!(merged.flagged, 2);
    assert_eq!(artist_text(&lib, lib.h).await?, "X, Target");
    let h_credits: Vec<Uuid> =
        sqlx::query_scalar("SELECT artist_id FROM track_credits WHERE track_id = $1 ORDER BY position")
            .bind(lib.h)
            .fetch_all(&lib.owner)
            .await?;
    assert_eq!(h_credits, vec![lib.x, target]);
    Ok(())
}

/// What a share shows: its tracks, and how many tracks a query without the share's own
/// filters finds (straight from the table, so only row-level security limits it).
async fn seen_by_share(lib: &Library, share: &db::shares::ActiveShare) -> anyhow::Result<(Vec<Uuid>, i64)> {
    let mut tx = db::begin_share(&lib.app, share.id).await?;
    let tracks = db::shares::tracks(&mut tx, share).await?;
    let all: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks").fetch_one(&mut *tx).await?;
    let mut ids: Vec<Uuid> = tracks.iter().map(|t| t.id).collect();
    ids.sort();
    Ok((ids, all))
}

#[sqlx::test]
async fn shares_open_only_their_target(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;

    // bob shares album A (V and H).
    let mut tx = db::begin(&lib.owner, Some(lib.bob)).await?;
    let token_hash = crate::auth::token_hash("share token");
    db::shares::create(&mut tx, &token_hash, None, Some(lib.album), None).await?;
    tx.commit().await?;
    let share = db::shares::find_active(&lib.app, &token_hash).await?.unwrap();

    let mut v_and_h = vec![lib.v, lib.h];
    v_and_h.sort();
    assert_eq!(seen_by_share(&lib, &share).await?, (v_and_h, 2), "V and H (restricted, but bob may see it), not O");

    // bob leaves friends: his share no longer shows H.
    sqlx::query("DELETE FROM user_groups WHERE user_id = $1 AND group_id = $2")
        .bind(lib.bob)
        .bind(lib.friends)
        .execute(&lib.owner)
        .await?;
    assert_eq!(seen_by_share(&lib, &share).await?, (vec![lib.v], 1));

    // A share transaction for a share that doesn't exist sees nothing at all.
    let mut tx = db::begin_share(&lib.app, Uuid::now_v7()).await?;
    let all: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks").fetch_one(&mut *tx).await?;
    assert_eq!(all, 0);
    Ok(())
}
