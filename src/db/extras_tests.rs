//! Extras (`db::extras`, migration 0040). The library: see `rls_tests::setup` (V and H on
//! album A, H restricted to friends, which bob is in and carol isn't; O on no album).

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

use super::rls_tests::setup;
use crate::db;
use crate::db::extras::{About, Access, FileFacts, Link};

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[sqlx::test]
async fn extras_are_made_filled_linked_and_kept(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let (id, access) = db::extras::create(&mut tx, " V vocals ", " Vocals ", &About::default(), &[Link::Track(lib.v)], None, "").await?;
    assert_eq!(access, Access::default(), "V isn't restricted");
    db::blobs::register(&mut tx, SHA, "main", Some(3)).await?;
    let facts = FileFacts { codec: Some("pcm_s16le".into()), duration_ms: Some(1000), ..Default::default() };
    let file = db::extras::add_file(&mut tx, id, SHA, "vox.wav", 3, &facts, "").await?;
    let stored = db::extras::StoredAs { extra_id: id, name: "V vocals".into(), hidden: false };
    assert_eq!(db::extras::stored_as(&mut tx, SHA).await?, Some(stored), "the same bytes: not added again");
    assert_eq!(db::extras::stored_as(&mut tx, &"b".repeat(64)).await?, None);
    assert!(db::extras::link(&mut tx, id, Link::Artist(lib.both), "").await?);
    assert!(!db::extras::link(&mut tx, id, Link::Artist(lib.both), "").await?, "once");
    assert!(db::extras::edit(&mut tx, id, "V vocals (dry)", "vocals", &About::default(), "").await?);

    let extra = db::extras::get(&mut tx, id).await?.unwrap();
    assert_eq!((extra.name.as_str(), extra.kind.as_str()), ("V vocals (dry)", "vocals"));
    assert_eq!(extra.files.len(), 1);
    assert_eq!(extra.files[0].codec.as_deref(), Some("pcm_s16le"));
    assert_eq!(extra.links.iter().map(|l| (l.kind.as_str(), l.id)).collect::<Vec<_>>(), [("track", lib.v), ("artist", lib.both)]);
    assert_eq!(db::extras::linked_to(&mut tx, Link::Track(lib.v)).await?.len(), 1);
    assert!(db::extras::linked_to(&mut tx, Link::Track(lib.o)).await?.is_empty());

    // Taken off (a file, then the extra): kept, and back.
    assert!(db::extras::set_file_hidden(&mut tx, file, true, "wrong take").await?);
    assert!(db::extras::stored_as(&mut tx, SHA).await?.unwrap().hidden, "still there, taken off");
    assert!(db::extras::set_hidden(&mut tx, id, true, "").await?);
    assert!(db::extras::get(&mut tx, id).await?.unwrap().hidden);
    assert!(db::extras::set_hidden(&mut tx, id, false, "").await?);
    assert!(db::extras::unlink(&mut tx, id, Link::Artist(lib.both), "").await?);
    let what: Vec<String> = db::extras::history(&mut tx, id).await?.into_iter().map(|l| l.what).collect();
    assert_eq!(what, ["unlinked", "brought back", "taken off", "file taken off", "edited", "linked", "file added", "linked", "made"]);
    assert!(db::extras::create(&mut tx, " ", "", &About::default(), &[], None, "").await.is_err(), "a name is needed");
    tx.commit().await?;

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert!(sqlx::query("DELETE FROM extras").execute(&mut *tx).await.is_err(), "never deleted");
    Ok(())
}

#[sqlx::test]
async fn extras_are_seen_like_their_tracks(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    // Bob (friends) attaches vocals to H: restricted like H, to friends.
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let (hidden, access) = db::extras::create(&mut tx, "H vocals", "vocals", &About::default(), &[Link::Track(lib.h)], None, "").await?;
    assert_eq!(access, Access { restricted: true, groups: vec![lib.friends] });
    // Another, on H and V, open to everyone (said so).
    let open = Access::default();
    let (both, _) = db::extras::create(&mut tx, "Shared sample", "sample", &About::default(), &[Link::Track(lib.h), Link::Track(lib.v)], Some(open), "").await?;
    assert!(db::extras::get(&mut tx, hidden).await?.is_some());
    tx.commit().await?;

    // Carol: not the restricted one, nor its links; the open one without H.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert!(db::extras::get(&mut tx, hidden).await?.is_none());
    assert!(db::extras::set_hidden(&mut tx, hidden, true, "").await.is_err());
    let shared = db::extras::get(&mut tx, both).await?.unwrap();
    assert_eq!(shared.links.iter().map(|l| l.id).collect::<Vec<_>>(), [lib.v]);
    assert!(db::extras::linked_to(&mut tx, Link::Track(lib.h)).await?.is_empty());
    drop(tx);

    // Carol can't restrict one to a group she isn't in (she'd lose it): said so.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let friends_only = Access { restricted: true, groups: vec![lib.friends] };
    let err = db::extras::create(&mut tx, "Mine", "clip", &About::default(), &[Link::Track(lib.v)], Some(friends_only), "").await.unwrap_err();
    assert!(err.to_string().contains("hide it from you"), "{err}");
    drop(tx);

    // Opened up to everyone: carol sees it (its link to H still not).
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    assert!(db::extras::set_access(&mut tx, hidden, &Access::default(), "for everyone").await?);
    tx.commit().await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let now = db::extras::get(&mut tx, hidden).await?.unwrap();
    assert!(!now.is_restricted && now.links.is_empty());
    Ok(())
}

#[sqlx::test]
async fn extras_are_in_the_catalog_and_its_deltas(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let version = |who| {
        let app = lib.app.clone();
        async move {
            let mut tx = db::begin(&app, Some(who)).await?;
            anyhow::Ok(db::library::version(&mut tx).await?)
        }
    };
    let since = version(lib.carol).await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let (open, _) = db::extras::create(&mut tx, "Sample", "sample", &About::default(), &[Link::Track(lib.v), Link::Track(lib.h)], None, "").await?;
    tx.commit().await?;

    // Carol: the new extra in the delta, its row without H (she can't see it).
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let changes = db::library::changes_since(&mut tx, &since).await?;
    assert_eq!(changes.extras, [open]);
    let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&db::library::extras_json(&mut tx, None).await?)?;
    let links = db::library::EXTRA_COLUMNS.iter().position(|c| *c == "links").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][links], serde_json::json!([["track", lib.v]]));
    drop(tx);

    // Restricted: a delta, which says it's gone for carol (not the whole catalog).
    let since = version(lib.carol).await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    db::extras::set_access(&mut tx, open, &Access { restricted: true, groups: vec![lib.friends] }, "").await?;
    tx.commit().await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let changes = db::library::changes_since(&mut tx, &since).await?;
    assert!(!changes.everything);
    assert_eq!(db::library::unseen(&mut tx, &since, &[], &[], &changes.extras).await?.2, [open]);
    assert_eq!(db::library::extras_json(&mut tx, None).await?, "[]");
    Ok(())
}

#[sqlx::test]
async fn a_track_becomes_an_extra_and_a_track_again(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let sees = |who, track| {
        let app = lib.app.clone();
        async move {
            let mut tx = db::begin(&app, Some(who)).await?;
            let seen: Option<uuid::Uuid> = sqlx::query_scalar("SELECT id FROM tracks WHERE id = $1").bind(track).fetch_optional(&mut *tx).await?;
            anyhow::Ok(seen.is_some())
        }
    };
    let album_seen = |who| {
        let app = lib.app.clone();
        let album = lib.album;
        async move {
            let mut tx = db::begin(&app, Some(who)).await?;
            let seen: Option<uuid::Uuid> = sqlx::query_scalar("SELECT id FROM albums WHERE id = $1").bind(album).fetch_optional(&mut *tx).await?;
            anyhow::Ok(seen.is_some())
        }
    };
    // (The fixture's files aren't in the blob store; real ones always are, see 0018.)
    sqlx::query("INSERT INTO blobs (sha256, volume, size_bytes) SELECT sha256, 'main', size_bytes FROM files ON CONFLICT DO NOTHING")
        .execute(&lib.owner)
        .await?;
    let since = {
        let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
        db::library::version(&mut tx).await?
    };
    // Carol can't turn H into anything: she doesn't see it.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert!(db::extras::track_to_extra(&mut tx, lib.h, "vocals", None, &About::default(), None, "").await.is_err());
    drop(tx);

    // O becomes the vocals of V.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let made = db::extras::track_to_extra(&mut tx, lib.o, "vocals", None, &About::default(), Some(lib.v), "stems imported as a track").await?;
    tx.commit().await?;
    assert_eq!((made.name.as_str(), made.kind.as_str()), ("Other", "vocals"));
    assert_eq!(made.links.iter().map(|l| (l.kind.as_str(), l.id)).collect::<Vec<_>>(), [("track", lib.v), ("artist", lib.both)]);
    assert_eq!(made.access, Access::default());
    assert!(made.albums.is_empty());
    assert!(made.source.contains("custom"), "the track's source: {}", made.source);
    for who in [lib.carol, lib.bob, lib.alice] {
        assert!(!sees(who, lib.o).await?, "nobody sees the track, admins neither");
    }
    // Its old link leads to the extra.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert_eq!(db::extras::became(&mut tx, lib.o).await?, Some(made.extra_id));
    assert_eq!(db::extras::became(&mut tx, lib.v).await?, None, "still a track");
    drop(tx);
    let o_sha: String = sqlx::query_scalar("SELECT f.sha256 FROM tracks t JOIN files f ON f.id = t.file_id WHERE t.id = $1").bind(lib.o).fetch_one(&lib.owner).await?;
    let extra_sha: String = sqlx::query_scalar("SELECT sha256 FROM extra_files WHERE extra_id = $1").bind(made.extra_id).fetch_one(&lib.owner).await?;
    assert_eq!(o_sha, extra_sha, "the same bytes");
    // O wasn't restricted: a small delta says it's gone (not the whole catalog).
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let changes = db::library::changes_since(&mut tx, &since).await?;
    assert!(!changes.everything);
    assert_eq!(db::library::unseen(&mut tx, &since, &changes.tracks, &changes.albums, &[]).await?.0, [lib.o]);
    assert!(changes.artists.contains(&lib.both), "its artist's counts");
    assert!(!db::library::tracks_json(&mut tx, None).await?.contains(&lib.o.to_string()));
    let since = db::library::version(&mut tx).await?;
    drop(tx);

    // V and H (album A): V becomes an extra, then H. With no track left, A is gone for
    // everyone; back, and it's there again.
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let v_extra = db::extras::track_to_extra(&mut tx, lib.v, "clip", Some("V clip"), &About::default(), None, "").await?;
    assert_eq!(v_extra.albums.iter().map(|a| (a.title.as_str(), a.alone)).collect::<Vec<_>>(), [("Album A", false)]);
    tx.commit().await?;
    // A is left with H only (restricted): gone for carol, said in the delta.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let changes = db::library::changes_since(&mut tx, &since).await?;
    assert!(!changes.everything);
    let (tracks, albums, _) = db::library::unseen(&mut tx, &since, &changes.tracks, &changes.albums, &[]).await?;
    assert!(tracks.contains(&lib.v) && albums.contains(&lib.album), "{tracks:?} {albums:?}");
    drop(tx);
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let h_extra = db::extras::track_to_extra(&mut tx, lib.h, "clip", None, &About::default(), None, "").await?;
    assert_eq!(h_extra.access, Access { restricted: true, groups: vec![lib.friends] }, "seen like H");
    assert!(h_extra.albums[0].alone);
    tx.commit().await?;
    assert!(!album_seen(lib.alice).await?, "an album of extras only is hidden, from admins too");
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    assert_eq!(db::extras::back_to_track(&mut tx, h_extra.extra_id, "").await?, lib.h);
    tx.commit().await?;
    // (Its own transaction: a failed statement aborts the one it's in.)
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    assert!(db::extras::back_to_track(&mut tx, h_extra.extra_id, "").await.is_err(), "once");
    drop(tx);
    assert!(sees(lib.bob, lib.h).await?, "H is back");
    let flags: (bool, bool) = sqlx::query_as("SELECT all_tracks_restricted, all_tracks_extras FROM albums WHERE id = $1").bind(lib.album).fetch_one(&lib.owner).await?;
    assert!(album_seen(lib.bob).await?, "A is back (flags {flags:?})");
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    assert!(db::extras::get(&mut tx, h_extra.extra_id).await?.unwrap().hidden, "the extra is taken off (kept)");
    Ok(())
}

/// A track alone on its single and that single are one release: an extra linked to one
/// is linked to both, and unlinked from both.
#[sqlx::test]
async fn an_extra_goes_with_a_track_and_its_single(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let single = db::review_bulk::make_singles(&mut tx, &[lib.o], "").await?.made[0].album_id;
    let (id, _) = db::extras::create(&mut tx, "O vocals", "vocals", &About::default(), &[Link::Track(lib.o)], None, "").await?;
    let linked = |extra: db::extras::Extra| extra.links.iter().map(|l| (l.kind.clone(), l.id)).collect::<Vec<_>>();
    let extra = db::extras::get(&mut tx, id).await?.unwrap();
    assert_eq!(linked(extra), [("track".to_string(), lib.o), ("album".to_string(), single)]);
    // V is on album A with H: A isn't a single of V's alone, so it doesn't go along.
    db::extras::link(&mut tx, id, Link::Track(lib.v), "").await?;
    assert_eq!(db::extras::get(&mut tx, id).await?.unwrap().links.len(), 3);
    // Unlinked from the single: its track goes too.
    assert!(db::extras::unlink(&mut tx, id, Link::Album(single), "").await?);
    assert_eq!(linked(db::extras::get(&mut tx, id).await?.unwrap()), [("track".to_string(), lib.v)]);
    Ok(())
}

/// A track alone on its single, turned into an extra: the delta says both are gone.
#[sqlx::test]
async fn a_single_goes_with_its_track_in_a_small_delta(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    sqlx::query("INSERT INTO blobs (sha256, volume, size_bytes) SELECT sha256, 'main', size_bytes FROM files ON CONFLICT DO NOTHING")
        .execute(&lib.owner)
        .await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let single = db::review_bulk::make_singles(&mut tx, &[lib.o], "").await?.made[0].album_id;
    tx.commit().await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let since = db::library::version(&mut tx).await?;
    db::extras::track_to_extra(&mut tx, lib.o, "vocals", None, &About::default(), Some(lib.v), "").await?;
    tx.commit().await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let changes = db::library::changes_since(&mut tx, &since).await?;
    assert!(!changes.everything);
    let (tracks, albums, _) = db::library::unseen(&mut tx, &since, &changes.tracks, &changes.albums, &[]).await?;
    assert_eq!((tracks, albums), (vec![lib.o], vec![single]));
    Ok(())
}

/// Tracks merged: the kept one shows the extras of the other (the links stay where they
/// were made), and undoing the merge gives them back.
#[sqlx::test]
async fn extras_follow_a_track_merge(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let (extra, _) = db::extras::create(&mut tx, "O stems", "stems", &About::default(), &[Link::Track(lib.o)], None, "").await?;
    let merged = db::revisions::merge_tracks(&mut tx, lib.v, lib.o, &Default::default(), None, None, "").await?;
    let on_v = db::extras::linked_to(&mut tx, Link::Track(lib.v)).await?;
    assert_eq!(on_v.iter().map(|e| e.id).collect::<Vec<_>>(), [extra]);
    assert_eq!(on_v[0].links.iter().map(|l| l.id).collect::<Vec<_>>(), [lib.v], "shown on the kept track");
    let rows = db::library::extras_json(&mut tx, None).await?;
    assert!(rows.contains(&lib.v.to_string()) && !rows.contains(&lib.o.to_string()));
    // The merge undone: back on O, not on V.
    db::revisions::undo_track_merge(&mut tx, merged.merge_id, "").await?;
    assert_eq!(db::extras::linked_to(&mut tx, Link::Track(lib.o)).await?.len(), 1);
    assert!(db::extras::linked_to(&mut tx, Link::Track(lib.v)).await?.is_empty());
    // Merged again: "Not on this track" from V undoes the link made to O.
    db::revisions::merge_tracks(&mut tx, lib.v, lib.o, &Default::default(), None, None, "").await?;
    assert!(db::extras::unlink(&mut tx, extra, Link::Track(lib.v), "").await?);
    assert!(db::extras::linked_to(&mut tx, Link::Track(lib.v)).await?.is_empty());
    Ok(())
}

/// Linked to a grouping (a collective's stems, say): on its page, in the catalog, undone.
#[sqlx::test]
async fn extras_are_linked_to_groupings(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let crew = db::groupings::create(&mut tx, "collective", "Tide Crew", "", "").await?;
    let (id, access) = db::extras::create(&mut tx, "Crew stems", "stems", &About::default(), &[Link::Grouping(crew)], None, "").await?;
    assert_eq!(access, Access::default(), "no track: seen by everyone");
    assert!(!db::extras::link(&mut tx, id, Link::Grouping(crew), "").await?, "once");
    assert!(db::extras::link(&mut tx, id, Link::Grouping(uuid::Uuid::now_v7()), "").await.is_err(), "a grouping that's there");

    let extra = db::extras::get(&mut tx, id).await?.unwrap();
    assert_eq!(extra.links, [db::extras::Linked { kind: "grouping".into(), id: crew, title: "Tide Crew".into() }]);
    let on_crew = db::groupings::detail(&mut tx, crew).await?.unwrap().extras;
    assert_eq!(on_crew.iter().map(|e| e.id).collect::<Vec<_>>(), [id]);
    let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&db::library::extras_json(&mut tx, None).await?)?;
    let links = db::library::EXTRA_COLUMNS.iter().position(|c| *c == "links").unwrap();
    assert_eq!(rows[0][links], serde_json::json!([["grouping", crew]]));

    assert!(db::extras::unlink(&mut tx, id, Link::Grouping(crew), "").await?);
    assert!(db::extras::linked_to(&mut tx, Link::Grouping(crew)).await?.is_empty());
    Ok(())
}

/// Extras that are one thing, merged: files and links move over (what the other has already
/// stays behind), the old ones go to the trash, taking them out undoes it, purging keeps
/// the tombstone.
#[sqlx::test]
async fn extras_are_merged_and_the_merge_undone(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let other_sha = "b".repeat(64);
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    db::blobs::register(&mut tx, SHA, "main", Some(3)).await?;
    db::blobs::register(&mut tx, &other_sha, "main", Some(3)).await?;
    let (keep, _) = db::extras::create(&mut tx, "V vocals", "vocals", &About::default(), &[Link::Track(lib.v)], None, "").await?;
    let (twin, _) = db::extras::create(&mut tx, "V vocals again", "vocals", &About::default(), &[Link::Track(lib.v), Link::Artist(lib.both)], None, "").await?;
    let (stems, _) = db::extras::create(&mut tx, "V stems", "stems", &About::default(), &[Link::Artist(lib.both)], None, "").await?;
    let facts = FileFacts::default();
    let kept_file = db::extras::add_file(&mut tx, keep, SHA, "vox.wav", 3, &facts, "").await?;
    let twin_file = db::extras::add_file(&mut tx, twin, SHA, "vox copy.wav", 3, &facts, "").await?;
    let stems_file = db::extras::add_file(&mut tx, stems, &other_sha, "stems.zip", 3, &facts, "").await?;

    // Not into itself; not while they're seen by different people.
    assert!(db::extras::merge(&mut tx, keep, &[keep], "").await.is_err());
    assert!(db::extras::merge(&mut tx, keep, &[], "").await.is_err());
    let before = db::extras::history(&mut tx, keep).await?.len();
    let friends = Access { restricted: true, groups: vec![lib.friends] };
    assert!(db::extras::set_access(&mut tx, stems, &friends, "").await?);
    let err = db::extras::merge(&mut tx, keep, &[stems], "").await.unwrap_err();
    assert!(err.to_string().contains("aren't seen by the same people"), "{err}");
    assert!(db::extras::set_access(&mut tx, stems, &Access::default(), "").await?);

    let report = db::extras::merge(&mut tx, keep, &[twin, stems], "same thing").await?;
    assert_eq!(report.merged.len(), 2);
    let twin_report = report.merged.iter().find(|m| m.id == twin).unwrap();
    assert_eq!((twin_report.files_moved.len(), twin_report.files_duplicate.len()), (0, 1), "the same bytes stay behind");
    assert_eq!((twin_report.links_moved.len(), twin_report.links_duplicate.len()), (1, 1));
    let stems_report = report.merged.iter().find(|m| m.id == stems).unwrap();
    assert_eq!(stems_report.files_moved, [db::extras::FileMoved { id: stems_file, name: "stems.zip".into() }]);

    let kept = db::extras::get(&mut tx, keep).await?.unwrap();
    assert_eq!(kept.files.iter().map(|f| f.id).collect::<Vec<_>>(), [stems_file, kept_file]);
    assert_eq!(kept.links.iter().map(|l| (l.kind.as_str(), l.id)).collect::<Vec<_>>(), [("track", lib.v), ("artist", lib.both)]);
    let old = db::extras::get(&mut tx, twin).await?.unwrap();
    assert!(old.hidden && old.merged_into == Some(keep) && old.links.is_empty());
    assert_eq!(old.files.iter().map(|f| f.id).collect::<Vec<_>>(), [twin_file]);
    assert_eq!(db::extras::linked_to(&mut tx, Link::Track(lib.v)).await?.iter().map(|e| e.id).collect::<Vec<_>>(), [keep]);
    assert!(db::extras::merge(&mut tx, keep, &[twin], "").await.is_err(), "merged already");
    assert!(db::extras::merge(&mut tx, twin, &[stems], "").await.is_err(), "not into one that's gone");
    assert!(db::trash::list(&mut tx).await?.iter().filter(|i| i.kind == "extra").count() == 2);
    let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&db::library::extras_json(&mut tx, None).await?)?;
    let hidden = db::library::EXTRA_COLUMNS.iter().position(|c| *c == "hidden").unwrap();
    assert_eq!(rows.iter().filter(|r| r[hidden] == serde_json::json!(true)).count(), 2, "the catalog drops them");
    assert_eq!(db::extras::history(&mut tx, keep).await?.len(), before + 2, "a line for each extra merged in");
    tx.commit().await?;

    // Taken out of the trash: its file and link back, the other's too.
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    assert!(db::trash::restore(&mut tx, db::trash::Kind::Extra, stems).await?);
    assert!(!db::trash::restore(&mut tx, db::trash::Kind::Extra, stems).await?, "once");
    let back = db::extras::get(&mut tx, stems).await?.unwrap();
    assert!(!back.hidden && back.merged_into.is_none());
    assert_eq!(back.files.iter().map(|f| f.id).collect::<Vec<_>>(), [stems_file]);
    assert_eq!(back.links.iter().map(|l| l.id).collect::<Vec<_>>(), [lib.both]);
    assert_eq!(db::extras::get(&mut tx, keep).await?.unwrap().files.len(), 1);
    tx.commit().await?;

    // Purged after the grace period: a tombstone, nothing to take back.
    sqlx::query("UPDATE extras SET trashed_at = now() - interval '8 days' WHERE id = $1").bind(twin).execute(&lib.owner).await?;
    let dir = tempfile::tempdir()?;
    let store = crate::storage::BlobStore::open(dir.path())?;
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let purged = db::trash::purge(&mut tx, &store, &[(db::trash::Kind::Extra, twin)], &twin.to_string()[..8]).await?;
    assert_eq!((purged.items, purged.delete_blobs.len()), (1, 0));
    assert!(!db::trash::restore(&mut tx, db::trash::Kind::Extra, twin).await?);
    assert!(db::extras::get(&mut tx, twin).await?.is_some());
    assert!(!db::trash::list(&mut tx).await?.iter().any(|i| i.id == twin));
    Ok(())
}
