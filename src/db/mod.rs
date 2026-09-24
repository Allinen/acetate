//! Database access. Queries are checked against the real schema at compile time
//! by the `sqlx::query!` macros (they read `DATABASE_URL` from `.env`).

pub mod catalog;
pub mod ingest;
pub mod sources;
pub mod tracks;
