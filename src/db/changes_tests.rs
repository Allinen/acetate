//! The changelist (`db::changes`). The library: see `rls_tests::setup` (V and H on album A,
//! H restricted to friends, V a remix of H).

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

use super::rls_tests::setup;
use crate::db;

#[sqlx::test]
async fn the_changelist_has_edits_newest_first_and_imports_when_asked(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;

    // Without the imports: the tracks' facts as set (V's last), V's after H's own.
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let edits = db::changes::recent(&mut tx, None, 50, false).await?.changes;
    let mut names: Vec<_> = edits.iter().map(|c| (c.name.as_str(), c.kind.as_str(), c.source.as_str())).collect();
    names.sort();
    assert_eq!(names, [("Song H", "external", "edit"), ("Song V", "external", "edit")]);
    assert!(edits[0].id > edits[1].id);
    let v = edits.iter().find(|c| c.name == "Song V").unwrap();
    assert_eq!(v.entity, "track");
    assert!(v.before.is_some(), "V's facts were written once before");
    assert_eq!(v.data["relations"].as_array().map(Vec::len), Some(1), "bob may see H");

    // With them: the tracks' and the album's first revisions as well, the newest first;
    // and paging by the last one's id goes on from there.
    let all = db::changes::recent(&mut tx, None, 50, true).await?.changes;
    assert!(all.len() > 4);
    assert!(all.windows(2).all(|w| w[0].id > w[1].id));
    assert!(all.iter().any(|c| c.entity == "album" && c.name == "Album A" && c.before.is_none()));
    let first = db::changes::recent(&mut tx, None, 2, true).await?.changes;
    let next = db::changes::recent(&mut tx, Some(first[1].id), 2, true).await?.changes;
    assert_eq!(next.iter().map(|c| c.id).collect::<Vec<_>>(), all[2..4].iter().map(|c| c.id).collect::<Vec<_>>());
    drop(tx);

    // Carol can't see H: no changes of it, and V's don't mention it.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let edits = db::changes::recent(&mut tx, None, 50, false).await?.changes;
    assert_eq!(edits.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Song V"]);
    assert_eq!(edits[0].data["relations"].as_array().map(Vec::len), Some(0), "H is hidden from carol");
    let all = db::changes::recent(&mut tx, None, 50, true).await?.changes;
    assert!(all.iter().all(|c| c.name != "Song H"));
    Ok(())
}
