//! The vocabulary (`db::vocabulary`, migration 0067). The library: see `rls_tests::setup`.

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

use super::rls_tests::setup;
use crate::db;
use crate::db::vocabulary::{Change, NewWord};

fn new(list: &str, word: &str) -> NewWord {
    NewWord { list: list.into(), word: word.into(), label: String::new(), description: String::new(), position: 0, inverse: None, template: None, inverse_template: None, mark: None, substring: false }
}

fn change() -> Change {
    Change { rename_to: None, label: None, description: None, position: None, template: None, mark: None }
}

#[sqlx::test]
async fn words_are_added_renamed_and_removed_but_the_locked_ones_stay(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;

    // Locked: can't be removed or renamed, can be described.
    let err = db::vocabulary::remove(&mut tx, "credit_role", "primary").await.unwrap_err().to_string();
    assert!(err.contains("built around"), "{err}");
    let rename = Change { rename_to: Some("main".into()), ..change() };
    assert!(db::vocabulary::change(&mut tx, "credit_role", "primary", rename).await.unwrap_err().to_string().contains("built around"));
    db::vocabulary::change(&mut tx, "credit_role", "primary", Change { description: Some("the artist of it".into()), ..change() }).await?;

    // A new one, in use, renamed with its users, then not in use, removed.
    db::vocabulary::add(&mut tx, new("status", "bootleg")).await?;
    assert!(db::vocabulary::add(&mut tx, new("status", "bootleg")).await.is_err(), "once");
    sqlx::query("UPDATE tracks SET status = 'bootleg' WHERE id = $1").bind(lib.v).execute(&mut *tx).await?;
    assert!(db::vocabulary::remove(&mut tx, "status", "bootleg").await.unwrap_err().to_string().contains("in use"));
    db::vocabulary::change(&mut tx, "status", "bootleg", Change { rename_to: Some("live recording".into()), ..change() }).await?;
    let status: String = sqlx::query_scalar("SELECT status FROM tracks WHERE id = $1").bind(lib.v).fetch_one(&mut *tx).await?;
    assert_eq!(status, "live recording", "the track follows");
    sqlx::query("UPDATE tracks SET status = 'released' WHERE id = $1").bind(lib.v).execute(&mut *tx).await?;
    db::vocabulary::remove(&mut tx, "status", "live recording").await?;
    let all = db::vocabulary::all(&mut tx).await?;
    assert!(!all.iter().any(|w| w.word == "live recording"));
    assert!(all.iter().any(|w| w.list == "status" && w.word == "released" && w.locked));
    tx.commit().await?;
    Ok(())
}

#[sqlx::test]
async fn relation_kinds_come_in_pairs(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let mut cover = new("relation_kind", "cover_of");
    assert!(db::vocabulary::add(&mut tx, NewWord { inverse: Some("cover_to".into()), ..new("relation_kind", "cover_of") }).await.is_err(), "needs its sentence");
    cover.inverse = Some("cover_to".into());
    cover.template = Some("%A is a cover of %B".into());
    cover.inverse_template = Some("%B is a cover of %A".into());
    db::vocabulary::add(&mut tx, cover).await?;
    let mut mirror = new("relation_kind", "mirror");
    mirror.inverse = Some("mirror".into());
    mirror.template = Some("%A mirrors %B".into());
    db::vocabulary::add(&mut tx, mirror).await?;
    let all = db::vocabulary::all(&mut tx).await?;
    let inverse = |w: &str| all.iter().find(|x| x.list == "relation_kind" && x.word == w).and_then(|x| x.inverse.clone());
    assert_eq!((inverse("cover_of").as_deref(), inverse("cover_to").as_deref(), inverse("mirror").as_deref()), (Some("cover_to"), Some("cover_of"), Some("mirror")));
    // Renamed: its inverse follows.
    db::vocabulary::change(&mut tx, "relation_kind", "cover_of", Change { rename_to: Some("version_of".into()), ..change() }).await?;
    let all = db::vocabulary::all(&mut tx).await?;
    assert_eq!(all.iter().find(|x| x.word == "cover_to").and_then(|x| x.inverse.clone()).as_deref(), Some("version_of"));
    // Removed together.
    db::vocabulary::remove(&mut tx, "relation_kind", "version_of").await?;
    let all = db::vocabulary::all(&mut tx).await?;
    assert!(!all.iter().any(|x| x.word == "version_of" || x.word == "cover_to"));
    tx.commit().await?;
    Ok(())
}

#[sqlx::test]
async fn extras_have_the_libraries_kinds(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let about = db::extras::About::default();
    let err = db::extras::create(&mut tx, "Ref", "reference mix", &about, &[], None, "").await.unwrap_err().to_string();
    assert!(err.contains("isn't a kind of extra"), "{err}");
    db::vocabulary::add(&mut tx, new("extra_kind", "reference")).await?;
    db::extras::create(&mut tx, "Ref", "reference", &about, &[], None, "").await?;
    Ok(())
}
