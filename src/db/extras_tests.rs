//! Extras (`db::extras`, migration 0040). The library: see `rls_tests::setup` (V and H on
//! album A, H restricted to friends, which bob is in and carol isn't; O on no album).

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

use super::rls_tests::setup;
use crate::db;
use crate::db::extras::{Access, FileFacts, Link};

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[sqlx::test]
async fn extras_are_made_filled_linked_and_kept(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let (id, access) = db::extras::create(&mut tx, " V vocals ", " Vocals ", &[Link::Track(lib.v)], None, "").await?;
    assert_eq!(access, Access::default(), "V isn't restricted");
    db::blobs::register(&mut tx, SHA, "main", Some(3)).await?;
    let facts = FileFacts { codec: Some("pcm_s16le".into()), duration_ms: Some(1000), ..Default::default() };
    let file = db::extras::add_file(&mut tx, id, SHA, "vox.wav", 3, &facts, "").await?;
    assert!(db::extras::link(&mut tx, id, Link::Artist(lib.both), "").await?);
    assert!(!db::extras::link(&mut tx, id, Link::Artist(lib.both), "").await?, "once");
    assert!(db::extras::edit(&mut tx, id, "V vocals (dry)", "vocals", "").await?);

    let extra = db::extras::get(&mut tx, id).await?.unwrap();
    assert_eq!((extra.name.as_str(), extra.kind.as_str()), ("V vocals (dry)", "vocals"));
    assert_eq!(extra.files.len(), 1);
    assert_eq!(extra.files[0].codec.as_deref(), Some("pcm_s16le"));
    assert_eq!(extra.links.iter().map(|l| (l.kind.as_str(), l.id)).collect::<Vec<_>>(), [("track", lib.v), ("artist", lib.both)]);
    assert_eq!(db::extras::linked_to(&mut tx, Link::Track(lib.v)).await?.len(), 1);
    assert!(db::extras::linked_to(&mut tx, Link::Track(lib.o)).await?.is_empty());

    // Taken off (a file, then the extra): kept, and back.
    assert!(db::extras::set_file_hidden(&mut tx, file, true, "wrong take").await?);
    assert!(db::extras::set_hidden(&mut tx, id, true, "").await?);
    assert!(db::extras::get(&mut tx, id).await?.unwrap().hidden);
    assert!(db::extras::set_hidden(&mut tx, id, false, "").await?);
    assert!(db::extras::unlink(&mut tx, id, Link::Artist(lib.both), "").await?);
    let what: Vec<String> = db::extras::history(&mut tx, id).await?.into_iter().map(|l| l.what).collect();
    assert_eq!(what, ["unlinked", "brought back", "taken off", "file taken off", "edited", "linked", "file added", "linked", "made"]);
    assert!(db::extras::create(&mut tx, " ", "", &[], None, "").await.is_err(), "a name is needed");
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
    let (hidden, access) = db::extras::create(&mut tx, "H vocals", "vocals", &[Link::Track(lib.h)], None, "").await?;
    assert_eq!(access, Access { restricted: true, groups: vec![lib.friends] });
    // Another, on H and V, open to everyone (said so).
    let open = Access::default();
    let (both, _) = db::extras::create(&mut tx, "Shared sample", "sample", &[Link::Track(lib.h), Link::Track(lib.v)], Some(open), "").await?;
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
    let err = db::extras::create(&mut tx, "Mine", "clip", &[Link::Track(lib.v)], Some(friends_only), "").await.unwrap_err();
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
    let (open, _) = db::extras::create(&mut tx, "Sample", "sample", &[Link::Track(lib.v), Link::Track(lib.h)], None, "").await?;
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

    // Restricted: the whole catalog again (nothing says it's gone).
    let since = version(lib.carol).await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    db::extras::set_access(&mut tx, open, &Access { restricted: true, groups: vec![lib.friends] }, "").await?;
    tx.commit().await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert!(db::library::changes_since(&mut tx, &since).await?.everything);
    assert_eq!(db::library::extras_json(&mut tx, None).await?, "[]");
    Ok(())
}
