//! Row-level security, end to end. Each test gets a fresh database with all migrations
//! (`#[sqlx::test]` creates it, and drops it afterwards), sets up a small library as the
//! owner (the system), then acts as different users through a pool that connects like the
//! server does (`test_app_pool`), calling the same `db::` functions the API calls.
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
    let app = db::test_app_pool(connect).await?;

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
            // With a description: a label alone would raise "source says too little".
            data: json!({ "label": "test", "description": "made by the tests" }),
        },
    )?;
    let snapshot = db::sources::add_snapshot(conn, SourceOwner::Track(track_id), &origin).await?;
    db::ingest::insert_file(
        conn,
        &db::ingest::NewFile {
            id: file_id,
            origin_snapshot_id: snapshot.snapshot_id,
            sha256: &crate::auth::token_hash(title),
            original_filename: Some(title),
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
            imported_from: None,
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

/// A track as a list of them shows it.
#[derive(serde::Serialize, sqlx::FromRow)]
pub struct Listed {
    pub id: Uuid,
    pub title: String,
    pub artist_display: String,
}

/// The tracks the user sees (not the merged ones, which only redirect), newest first: what
/// a list of tracks would show.
pub async fn list_tracks(conn: &mut sqlx::PgConnection) -> sqlx::Result<Vec<Listed>> {
    sqlx::query_as("SELECT t.id, t.title, t.artist_display FROM tracks t JOIN files f ON f.id = t.file_id WHERE t.merged_into IS NULL ORDER BY t.id DESC").fetch_all(conn).await
}

#[sqlx::test]
async fn server_connections_are_rls_bound(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let role: String = sqlx::query_scalar("SELECT current_user::text").fetch_one(&lib.app).await?;
    assert_eq!(role, db::APP_ROLE);
    db::check_row_level_security(&lib.owner).await?;
    Ok(())
}

/// What SQL injected into a query could do: not much. The server's login is the app role
/// itself, so there is no owner to go back to, and the one other role it can become changes
/// no tables.
#[sqlx::test]
async fn the_server_cannot_become_the_owner_or_change_the_schema(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect.clone()).await?;
    let owner_name = connect.get_username().to_string();
    let mut conn = lib.app.acquire().await?;
    let session_user: String = sqlx::query_scalar("SELECT session_user::text").fetch_one(&mut *conn).await?;
    assert_eq!(session_user, db::APP_ROLE, "logged in as the app role, not the owner");
    let superuser: String = sqlx::query_scalar("SELECT current_setting('is_superuser')").fetch_one(&mut *conn).await?;
    assert_eq!(superuser, "off");
    // Back to the owner: no.
    sqlx::query("RESET ROLE").execute(&mut *conn).await?;
    let still: String = sqlx::query_scalar("SELECT current_user::text").fetch_one(&mut *conn).await?;
    assert_eq!(still, db::APP_ROLE);
    assert!(sqlx::query(sqlx::AssertSqlSafe(format!("SET ROLE {owner_name}"))).execute(&mut *conn).await.is_err());
    // Nor change the schema, make roles, or run programs.
    assert!(sqlx::query("CREATE TABLE sneaky (x int)").execute(&mut *conn).await.is_err());
    assert!(sqlx::query("DROP TABLE tracks").execute(&mut *conn).await.is_err());
    assert!(sqlx::query("CREATE ROLE sneaky LOGIN").execute(&mut *conn).await.is_err());
    assert!(sqlx::query("COPY (SELECT 1) TO PROGRAM 'true'").execute(&mut *conn).await.is_err());

    // The role `as_system` becomes sees what is hidden, and changes no tables either.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let hidden_before: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks WHERE id = $1").bind(lib.h).fetch_one(&mut *tx).await?;
    assert_eq!(hidden_before, 0);
    let system = db::as_system(&mut tx).await?;
    let hidden_now: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks WHERE id = $1").bind(lib.h).fetch_one(&mut *tx).await?;
    assert_eq!(hidden_now, 1, "the system sees the restricted track");
    system.end(&mut tx).await?;
    let hidden_after: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks WHERE id = $1").bind(lib.h).fetch_one(&mut *tx).await?;
    assert_eq!(hidden_after, 0, "and the app doesn't again");
    tx.rollback().await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let system = db::as_system(&mut tx).await?;
    assert!(sqlx::query("DROP TABLE tracks").execute(&mut *tx).await.is_err());
    drop(system);
    Ok(())
}

#[sqlx::test]
async fn restricted_track_is_hidden_everywhere(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let hidden = [lib.h, lib.h_file, lib.x];
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;

    let list = list_tracks(&mut tx).await?;
    let mut ids: Vec<Uuid> = list.iter().map(|t| t.id).collect();
    ids.sort();
    let mut v_and_o = vec![lib.v, lib.o];
    v_and_o.sort();
    assert_eq!(ids, v_and_o);
    assert_mentions_none("track list", &list, &hidden);
    assert!(db::tracks::get_track(&mut tx, lib.h, true).await?.is_none());

    let files: i64 = sqlx::query_scalar("SELECT count(*) FROM files WHERE id = $1")
        .bind(lib.h_file)
        .fetch_one(&mut *tx)
        .await?;
    assert_eq!(files, 0, "H's file");

    // X is only credited on H. Artists are visible to everyone logged in (migration
    // 0027): carol sees the name, but none of its tracks.
    assert!(db::catalog::get_artist(&mut tx, lib.x, true).await?.is_some());
    assert!(db::catalog::artist_tracks(&mut tx, lib.x, None, 100).await?.is_empty());

    // Things that include V and H show V only.
    let album = db::catalog::get_album(&mut tx, lib.album, true).await?.unwrap();
    assert_eq!(album.tracks.len(), 1);
    assert_mentions_none("album", &album, &hidden);
    let both = db::catalog::artist_tracks(&mut tx, lib.both, None, 100).await?;
    assert_eq!(both.len(), 2, "V and O");
    let v = db::tracks::get_track(&mut tx, lib.v, true).await?.unwrap();
    assert!(v.relations.is_empty(), "the relation with H");
    assert_mentions_none("V's details and history", &v, &hidden);
    let flags = db::review::list(&mut tx, &db::review::Filter::default(), None, 100).await?;
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

#[sqlx::test]
async fn an_album_of_only_restricted_tracks_stays_hidden(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    // Album B has only T, restricted to friends; album C has T and a public track U.
    let mut tx = db::begin(&lib.owner, None).await?;
    let album = |title: &str| AlbumData { title: title.into(), ..Default::default() };
    let b = db::revisions::create_album(&mut tx, &album("Album B"), Source::Import, "").await?;
    let c = db::revisions::create_album(&mut tx, &album("Album C"), Source::Import, "").await?;
    let empty = db::revisions::create_album(&mut tx, &album("Empty"), Source::Import, "").await?;
    let (t, _) = make_track(&mut tx, "Song T", &[lib.x], Some(b)).await?;
    db::access::set(&mut tx, t, true, &[lib.friends], "test").await?;
    let (_u, _) = make_track(&mut tx, "Song U", &[lib.both], Some(c)).await?;
    sqlx::query("INSERT INTO album_tracks (album_id, track_id) VALUES ($1, $2)")
        .bind(c)
        .bind(t)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    let seen = |user: Uuid| {
        let lib = &lib;
        async move {
            let mut tx = db::begin(&lib.app, Some(user)).await?;
            let mut ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM albums WHERE id = ANY($1)")
                .bind(&[b, c, empty][..])
                .fetch_all(&mut *tx)
                .await?;
            ids.sort();
            anyhow::Ok(ids)
        }
    };
    let sorted = |mut ids: Vec<Uuid>| {
        ids.sort();
        ids
    };
    assert_eq!(seen(lib.carol).await?, sorted(vec![c, empty]), "carol: not B");
    assert_eq!(seen(lib.bob).await?, sorted(vec![b, c, empty]), "bob (friends)");
    assert_eq!(seen(lib.alice).await?, sorted(vec![b, c, empty]), "alice (admin)");

    // T made public: B is visible to all. Restricted again: hidden again.
    let mut tx = db::begin(&lib.owner, None).await?;
    db::access::set(&mut tx, t, false, &[], "test").await?;
    tx.commit().await?;
    assert_eq!(seen(lib.carol).await?, sorted(vec![b, c, empty]));
    let mut tx = db::begin(&lib.owner, None).await?;
    db::access::set(&mut tx, t, true, &[lib.friends], "test").await?;
    tx.commit().await?;
    assert_eq!(seen(lib.carol).await?, sorted(vec![c, empty]));

    // U taken off C: C has only T left, and is hidden from carol too.
    let mut tx = db::begin(&lib.owner, None).await?;
    sqlx::query("DELETE FROM album_tracks WHERE album_id = $1 AND track_id <> $2")
        .bind(c)
        .bind(t)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    assert_eq!(seen(lib.carol).await?, vec![empty]);
    Ok(())
}

#[sqlx::test]
async fn artists_are_visible_when_logged_in_and_through_a_share_only_its_own(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    // Z is only credited on a hidden track, then merged into X (also only on hidden H).
    let mut tx = db::begin(&lib.owner, None).await?;
    let z = db::ingest::find_or_create_artist(&mut tx, "Z").await?.id;
    let (t, _) = make_track(&mut tx, "Song T", &[z], None).await?;
    db::access::set(&mut tx, t, true, &[lib.friends], "test").await?;
    tx.commit().await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    db::revisions::merge_artist(&mut tx, z, lib.x, false, false, "").await?;
    tx.commit().await?;

    for user in [lib.bob, lib.carol] {
        let mut tx = db::begin(&lib.app, Some(user)).await?;
        assert_eq!(artists_seen(&mut tx, vec![z, lib.x, lib.both]).await?, 3, "user {user}");
    }
    // A share of O (credited to Both) shows Both, not X or Z.
    let mut tx = db::begin(&lib.owner, Some(lib.bob)).await?;
    let token_hash = crate::auth::token_hash("share of O");
    db::shares::create(&mut tx, &token_hash, Some(lib.o), None, None, false, None).await?;
    tx.commit().await?;
    let share = db::shares::find_active(&lib.app, &token_hash).await?.unwrap();
    let mut tx = db::begin_share(&lib.app, share.id).await?;
    assert_eq!(artists_seen(&mut tx, vec![z, lib.x]).await?, 0);
    assert_eq!(artists_seen(&mut tx, vec![lib.both]).await?, 1);
    Ok(())
}

/// How many of `ids` are visible artists.
async fn artists_seen(tx: &mut sqlx::PgConnection, ids: Vec<Uuid>) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT count(*) FROM artists WHERE id = ANY($1)").bind(ids).fetch_one(tx).await
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
    db::shares::create(&mut tx, &token_hash, None, Some(lib.album), None, false, None).await?;
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

#[sqlx::test]
async fn the_share_list_says_whether_a_link_still_works(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    // bob shares H (restricted to friends, which he's in) and O.
    let mut tx = db::begin(&lib.owner, Some(lib.bob)).await?;
    let share_h = db::shares::create(&mut tx, &crate::auth::token_hash("h"), Some(lib.h), None, None, false, None).await?;
    let share_o = db::shares::create(&mut tx, &crate::auth::token_hash("o"), Some(lib.o), None, None, false, None).await?;
    tx.commit().await?;
    let states = || async {
        // As carol would list them if she were an admin: she can't see H herself.
        let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
        let list = db::shares::list(&mut tx, Some(lib.bob)).await?;
        let state = |id| list.iter().find(|s| s.id == id).unwrap().state.clone();
        anyhow::Ok((state(share_h), state(share_o)))
    };
    assert_eq!(states().await?, ("active".into(), "active".into()), "judged through bob's eyes, not carol's");

    // bob leaves friends: H is hidden from him, so its link shows nothing.
    sqlx::query("DELETE FROM user_groups WHERE user_id = $1 AND group_id = $2")
        .bind(lib.bob)
        .bind(lib.friends)
        .execute(&lib.owner)
        .await?;
    assert_eq!(states().await?, ("target_hidden".into(), "active".into()));

    // bob may not share anymore: both stop.
    sqlx::query("DELETE FROM user_groups WHERE user_id = $1 AND group_id = (SELECT id FROM groups WHERE name = 'sharers')")
        .bind(lib.bob)
        .execute(&lib.owner)
        .await?;
    assert_eq!(states().await?, ("creator_cannot_share".into(), "creator_cannot_share".into()));

    let mut tx = db::begin(&lib.owner, Some(lib.bob)).await?;
    db::shares::revoke(&mut tx, share_o, None).await?;
    tx.commit().await?;
    assert_eq!(states().await?.1, "revoked");
    Ok(())
}

#[sqlx::test]
async fn cover_images_by_hash_follow_what_you_can_see(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    // The same image on V (visible to all) and H (hidden from carol); another only on H.
    let shared = "a".repeat(64);
    let hidden = "b".repeat(64);
    for (track, sha) in [(lib.v, &shared), (lib.h, &shared), (lib.h, &hidden)] {
        sqlx::query("INSERT INTO covers (track_id, blob_sha256, source) VALUES ($1, $2, 'upload')")
            .bind(track)
            .bind(sha)
            .execute(&lib.owner)
            .await?;
    }
    let found = |user: Uuid, hash: String| {
        let lib = &lib;
        async move {
            let mut tx = db::begin(&lib.app, Some(user)).await?;
            anyhow::Ok(db::covers::image_by_hash(&mut tx, &hash).await?.map(|i| i.blob_sha256))
        }
    };
    assert_eq!(found(lib.carol, shared.clone()).await?, Some(shared.clone()), "used by V too");
    assert_eq!(found(lib.carol, shared[..16].to_string()).await?, Some(shared.clone()), "by its start");
    assert_eq!(found(lib.carol, hidden.clone()).await?, None, "only on H");
    assert_eq!(found(lib.bob, hidden.clone()).await?, Some(hidden), "bob sees H");
    assert_eq!(found(lib.carol, "a".repeat(15)).await?, None, "too short to say");
    assert_eq!(found(lib.carol, "%".repeat(16)).await?, None, "not hex");
    // An image staged (not a cover yet): its stager's alone.
    let staged = "c".repeat(64);
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    db::covers::stage(&mut tx, &staged, Some("image/png"), None, None).await?;
    tx.commit().await?;
    assert_eq!(found(lib.carol, staged.clone()).await?, Some(staged.clone()), "carol staged it");
    assert_eq!(found(lib.bob, staged).await?, None, "not bob's");
    Ok(())
}
