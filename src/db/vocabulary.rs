//! The words Acetate knows (statuses, credit roles, album kinds, relation kinds, kinds of
//! review flag and of extra): one table, `vocabulary` (migration 0067). Some are `locked`,
//! the ones the code is built around; the rest are the library's own, and an admin adds,
//! describes, orders, renames and removes them here. `db::catalog::vocabularies` serves the
//! words to forms.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;

use super::revisions::EditRejected;

/// The lists (the `vocabulary.list` values), with what a word of each is called.
pub const LISTS: &[(&str, &str)] = &[
    ("status", "status"),
    ("credit_role", "credit role"),
    ("album_kind", "album kind"),
    ("relation_kind", "relation kind"),
    ("review_kind", "kind of review flag"),
    ("extra_kind", "kind of extra"),
    ("version_word", "version word"),
];

fn what(list: &str) -> &'static str {
    LISTS.iter().find(|(l, _)| *l == list).map_or("word", |(_, w)| w)
}

#[derive(Debug, Serialize)]
pub struct Word {
    pub list: String,
    pub word: String,
    pub label: String,
    pub description: String,
    pub locked: bool,
    pub position: i32,
    pub inverse: Option<String>,
    pub data: Value,
    /// How many rows use it (it can't be removed while any does).
    pub uses: i64,
}

/// Every word.
pub async fn all(conn: &mut PgConnection) -> sqlx::Result<Vec<Word>> {
    sqlx::query_as!(
        Word,
        r#"SELECT list, word, label, description, locked, position, inverse, data, vocabulary_uses(list, word) AS "uses!"
           FROM vocabulary ORDER BY list, position, word"#
    )
    .fetch_all(conn)
    .await
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewWord {
    pub list: String,
    pub word: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub position: i32,
    /// Relation kinds: the word for the other direction (the same word: a symmetric kind),
    /// made too if it's new.
    #[serde(default)]
    pub inverse: Option<String>,
    /// Relation kinds: the sentence, with %A and %B ("%A is a remix of %B").
    #[serde(default)]
    pub template: Option<String>,
    /// The sentence for the inverse, when it's a new word.
    #[serde(default)]
    pub inverse_template: Option<String>,
    /// Version words: what two titles with it agree on ("remastered" and "remaster" are both
    /// "remaster").
    #[serde(default)]
    pub mark: Option<String>,
    /// Version words: found anywhere in a title, not as words (for languages written
    /// without spaces between words).
    #[serde(default)]
    pub substring: bool,
}

fn invalid(message: impl Into<String>) -> anyhow::Error {
    EditRejected::Invalid(message.into()).into()
}

fn check_word(list: &str, word: &str) -> anyhow::Result<()> {
    if word.is_empty() || word != word.trim() || word.chars().count() > 60 {
        return Err(invalid("a word is 1 to 60 characters, with no spaces around it"));
    }
    if list == "extra_kind" && word != word.to_lowercase() {
        return Err(invalid("a kind of extra is in lowercase"));
    }
    Ok(())
}

/// Adds a word (two, for a new relation kind and its inverse). The user is `by`.
pub async fn add(conn: &mut PgConnection, new: NewWord) -> anyhow::Result<()> {
    let list = new.list.as_str();
    if !LISTS.iter().any(|(l, _)| *l == list) {
        return Err(invalid(format!("no such list: {list}")));
    }
    if list == "review_kind" {
        // Kinds of flag are named, and their key made from the name.
        let name = if new.label.trim().is_empty() { new.word.as_str() } else { new.label.as_str() };
        super::review::make_kind(conn, name).await?;
        return Ok(());
    }
    let word = new.word.as_str();
    check_word(list, word)?;
    let exists = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM vocabulary WHERE list = $1 AND word = $2) AS "e!""#, list, word).fetch_one(&mut *conn).await?;
    if exists {
        return Err(invalid(format!("there's a {} “{word}” already", what(list))));
    }
    if list == "relation_kind" {
        let inverse = new.inverse.as_deref().ok_or_else(|| invalid("a relation kind needs its inverse (the same word if it reads the same both ways)"))?;
        check_word(list, inverse)?;
        let template = new.template.as_deref().filter(|t| t.contains("%A") && t.contains("%B")).ok_or_else(|| invalid("a relation kind needs its sentence, with %A and %B in it (“%A is a remix of %B”)"))?;
        let inverse_exists = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM vocabulary WHERE list = $1 AND word = $2) AS "e!""#, list, inverse).fetch_one(&mut *conn).await?;
        insert(conn, list, word, (new.label.trim(), new.description.trim(), new.position), Some(inverse), json!({ "template": template })).await?;
        if inverse != word {
            if inverse_exists {
                return Err(invalid(format!("there's a relation kind “{inverse}” already: it has an inverse of its own")));
            }
            let inverse_template = new.inverse_template.as_deref().filter(|t| t.contains("%A") && t.contains("%B")).ok_or_else(|| invalid("the inverse needs its sentence too, with %A and %B (“%B is a remix of %A”)"))?;
            insert(conn, list, inverse, ("", "", new.position), Some(word), json!({ "template": inverse_template })).await?;
        }
        return Ok(());
    }
    if list == "version_word" {
        let mark = new.mark.as_deref().map(str::trim).filter(|m| !m.is_empty()).ok_or_else(|| invalid("a version word needs its mark (what versions with it have in common: “remaster”)"))?;
        if word != word.to_lowercase() {
            return Err(invalid("a version word is in lowercase"));
        }
        let data = if new.substring { json!({ "mark": mark, "substring": true }) } else { json!({ "mark": mark }) };
        return insert(conn, list, word, (new.label.trim(), new.description.trim(), new.position), None, data).await;
    }
    if new.inverse.is_some() || new.template.is_some() {
        return Err(invalid("only relation kinds have an inverse and a sentence"));
    }
    insert(conn, list, word, (new.label.trim(), new.description.trim(), new.position), None, json!({})).await
}

async fn insert(conn: &mut PgConnection, list: &str, word: &str, (label, description, position): (&str, &str, i32), inverse: Option<&str>, data: Value) -> anyhow::Result<()> {
    sqlx::query!(
        "INSERT INTO vocabulary (list, word, label, description, position, inverse, data, created_by, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, current_app_user(), now())",
        list,
        word,
        label,
        description,
        position,
        inverse,
        data
    )
    .execute(conn)
    .await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    /// A new word for it (everything that uses the word follows). Not for a locked one.
    #[serde(default)]
    pub rename_to: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub position: Option<i32>,
    /// A relation kind's sentence.
    #[serde(default)]
    pub template: Option<String>,
    /// A version word's mark.
    #[serde(default)]
    pub mark: Option<String>,
}

async fn find(conn: &mut PgConnection, list: &str, word: &str) -> anyhow::Result<(bool, Option<String>)> {
    let row = sqlx::query!("SELECT locked, inverse FROM vocabulary WHERE list = $1 AND word = $2", list, word)
        .fetch_optional(conn)
        .await?
        .ok_or_else(|| invalid(format!("there's no {} “{word}”", what(list))))?;
    Ok((row.locked, row.inverse))
}

/// Changes a word. A locked one can be described and ordered, not renamed.
pub async fn change(conn: &mut PgConnection, list: &str, word: &str, change: Change) -> anyhow::Result<()> {
    let (locked, _) = find(conn, list, word).await?;
    if let Some(label) = &change.label
        && list == "review_kind"
        && label.trim().is_empty()
    {
        return Err(invalid("a kind of flag needs a name"));
    }
    if let Some(template) = &change.template {
        if list != "relation_kind" {
            return Err(invalid("only relation kinds have a sentence"));
        }
        if !(template.contains("%A") && template.contains("%B")) {
            return Err(invalid("the sentence needs %A and %B in it"));
        }
        sqlx::query!("UPDATE vocabulary SET data = jsonb_set(data, '{template}', to_jsonb($3::text)) WHERE list = $1 AND word = $2", list, word, template).execute(&mut *conn).await?;
    }
    if let Some(mark) = change.mark.as_deref().map(str::trim) {
        if list != "version_word" || mark.is_empty() {
            return Err(invalid("only a version word has a mark, and it isn't empty"));
        }
        sqlx::query!("UPDATE vocabulary SET data = jsonb_set(data, '{mark}', to_jsonb($3::text)) WHERE list = $1 AND word = $2", list, word, mark).execute(&mut *conn).await?;
    }
    sqlx::query!(
        "UPDATE vocabulary SET label = COALESCE($3, label), description = COALESCE($4, description), position = COALESCE($5, position)
         WHERE list = $1 AND word = $2",
        list,
        word,
        change.label.as_deref().map(str::trim),
        change.description.as_deref().map(str::trim),
        change.position
    )
    .execute(&mut *conn)
    .await?;
    if let Some(new) = change.rename_to.as_deref().filter(|n| *n != word) {
        if locked {
            return Err(invalid(format!("the {} “{word}” is one the app is built around: it can't be renamed", what(list))));
        }
        if list == "review_kind" {
            return Err(invalid("a kind of flag is renamed by changing its name, not its key"));
        }
        check_word(list, new)?;
        sqlx::query!("SELECT rename_vocabulary($1, $2, $3)", list, word, new).execute(&mut *conn).await?;
    }
    Ok(())
}

/// Removes a word (and, for a relation kind, its inverse), unless it's locked or in use.
pub async fn remove(conn: &mut PgConnection, list: &str, word: &str) -> anyhow::Result<()> {
    let (locked, inverse) = find(conn, list, word).await?;
    if locked {
        return Err(invalid(format!("the {} “{word}” is one the app is built around: it can't be removed", what(list))));
    }
    let mut words = vec![word.to_string()];
    if let Some(inverse) = inverse.filter(|i| i != word) {
        words.push(inverse);
    }
    for w in &words {
        let uses = sqlx::query_scalar!(r#"SELECT vocabulary_uses($1, $2) AS "uses!""#, list, w).fetch_one(&mut *conn).await?;
        if uses > 0 {
            return Err(invalid(format!("the {} “{w}” is in use ({uses}): change those first", what(list))));
        }
    }
    // (Both halves of a relation kind in one statement: each is the other's inverse.)
    sqlx::query!("SET CONSTRAINTS ALL DEFERRED").execute(&mut *conn).await?;
    sqlx::query!("DELETE FROM vocabulary WHERE list = $1 AND word = ANY($2)", list, &words).execute(&mut *conn).await?;
    Ok(())
}
