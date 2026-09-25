-- Lookups that can use their indexes under row-level security.
--
-- Under RLS, Postgres checks a query's own conditions only after the policy's, unless it
-- can prove they don't leak anything about hidden rows: plain comparisons (`=` on uuid or
-- text) qualify, but `COALESCE(track_id, album_id, artist_id) = $1` and
-- `lower(name) = lower($1)` don't (COALESCE and lower() aren't marked LEAKPROOF). Such a
-- condition can't pick rows out of an index, so every row of the table is read and put
-- through the policy first: a lookup by id took seconds on a library still being imported
-- (and got slower with every file).
--
-- So: the owner of a source is looked up column by column (`track_id = $1 OR album_id = $1
-- OR artist_id = $1`, with an index on each), and names by a stored lowercase copy
-- (`name_lower = lower($1)`; lower() of a parameter is fine, it reads no row).
-- src/db/plan_tests.rs checks that these lookups use their indexes.

CREATE INDEX sources_track_idx ON sources (track_id) WHERE track_id IS NOT NULL;
CREATE INDEX sources_album_idx ON sources (album_id) WHERE album_id IS NOT NULL;
CREATE INDEX sources_artist_idx ON sources (artist_id) WHERE artist_id IS NOT NULL;
-- Only the owner's own (system) queries could use this one; the unique index on the same
-- expression stays, it's what keeps (owner, platform, id) unique.
DROP INDEX sources_owner_idx;

ALTER TABLE artists ADD COLUMN name_lower text GENERATED ALWAYS AS (lower(name)) STORED;
CREATE INDEX artists_name_lower_col_idx ON artists (name_lower);
DROP INDEX artists_name_lower_idx;

ALTER TABLE artist_aliases ADD COLUMN alias_lower text GENERATED ALWAYS AS (lower(alias)) STORED;
CREATE INDEX artist_aliases_alias_lower_col_idx ON artist_aliases (alias_lower);
DROP INDEX artist_aliases_alias_lower_idx;

ALTER TABLE albums ADD COLUMN title_lower text GENERATED ALWAYS AS (lower(title)) STORED;
CREATE INDEX albums_title_lower_col_idx ON albums (title_lower);
DROP INDEX albums_title_lower_idx;
