//! Read queries for artists and albums.

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::sources::Source;

#[derive(Debug, Serialize)]
pub struct ArtistDetail {
    pub id: Uuid,
    pub name: String,
    pub disambiguation: String,
    pub aliases: Vec<String>,
    pub sources: Vec<Source>,
}

pub async fn get_artist(pool: &PgPool, id: Uuid) -> sqlx::Result<Option<ArtistDetail>> {
    let Some(row) = sqlx::query!(
        r#"
        SELECT a.id, a.name, a.disambiguation,
               COALESCE(array_agg(al.alias ORDER BY al.alias) FILTER (WHERE al.id IS NOT NULL),
                        '{}') AS "aliases!"
        FROM artists a
        LEFT JOIN artist_aliases al ON al.artist_id = a.id
        WHERE a.id = $1
        GROUP BY a.id
        "#,
        id
    )
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };

    Ok(Some(ArtistDetail {
        id: row.id,
        name: row.name,
        disambiguation: row.disambiguation,
        aliases: row.aliases,
        sources: super::sources::list_for(pool, id).await?,
    }))
}

#[derive(Debug, Serialize)]
pub struct AlbumDetail {
    pub id: Uuid,
    pub title: String,
    pub kind: String,
    pub release_date: Option<String>,
    pub sources: Vec<Source>,
}

pub async fn get_album(pool: &PgPool, id: Uuid) -> sqlx::Result<Option<AlbumDetail>> {
    let Some(row) = sqlx::query!(
        "SELECT id, title, kind, release_date FROM albums WHERE id = $1",
        id
    )
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };

    Ok(Some(AlbumDetail {
        id: row.id,
        title: row.title,
        kind: row.kind,
        release_date: row.release_date,
        sources: super::sources::list_for(pool, id).await?,
    }))
}
