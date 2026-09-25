-- Credits point at aliases instead of carrying free text, and artists can be merged.
--
-- A credit is (artist, role, alias?): no alias = shown as the artist's name, otherwise
-- as that alias. Every spelling is an alias, case variants included ("mira vale" next to
-- "Mira Vale"), so the tags' exact spelling is kept.
--
-- Aliases keep their id forever, because revisions refer to them. Removing an alias from
-- an artist only hides it (`listed = false`). Merged artists and aliases stay as rows
-- that redirect (`merged_into`), so old revisions still resolve.

-- ============================================================================
-- Aliases
-- ============================================================================

ALTER TABLE artist_aliases
    ADD COLUMN listed      boolean NOT NULL DEFAULT true,
    ADD COLUMN merged_into uuid REFERENCES artist_aliases (id),
    ADD COLUMN created_at  timestamptz NOT NULL DEFAULT now(),
    ADD CONSTRAINT artist_aliases_artist_id_id_key UNIQUE (artist_id, id);

-- Case variants are separate aliases now; only exact duplicates (among listed aliases of
-- one artist) are refused.
DROP INDEX artist_aliases_unique_idx;
CREATE UNIQUE INDEX artist_aliases_listed_unique_idx ON artist_aliases (artist_id, alias)
    WHERE listed;

-- A merged artist redirects to the one it was merged into. `merged_alias_id`: the alias
-- its name became on that artist, if the name was kept.
ALTER TABLE artists
    ADD COLUMN merged_into     uuid REFERENCES artists (id),
    ADD COLUMN merged_alias_id uuid REFERENCES artist_aliases (id),
    ADD CONSTRAINT artists_not_merged_into_self CHECK (merged_into <> id);

CREATE INDEX artists_merged_into_idx ON artists (merged_into) WHERE merged_into IS NOT NULL;

-- ---- Every spelling used so far becomes an alias ----

-- Spellings that are in use now (current credits, and the latest revision of every variant
-- and album) are listed. Ones that only appear in older history are created unlisted, so
-- old revisions can still refer to them.
CREATE TEMP TABLE spellings (artist_id uuid, alias text, in_use boolean);

INSERT INTO spellings
SELECT artist_id, credited_as, true FROM track_credits WHERE credited_as IS NOT NULL
UNION ALL
SELECT artist_id, credited_as, true FROM album_credits WHERE credited_as IS NOT NULL;

INSERT INTO spellings
SELECT (c ->> 'artist_id')::uuid, c ->> 'credited_as',
       r.id = (SELECT r2.id FROM track_revisions r2 WHERE r2.variant_id = r.variant_id
               ORDER BY r2.id DESC LIMIT 1)
FROM track_revisions r, jsonb_array_elements(r.data -> 'credits') c
WHERE r.kind = 'internal' AND c ? 'credited_as';

INSERT INTO spellings
SELECT (c ->> 'artist_id')::uuid, c ->> 'credited_as',
       r.id = (SELECT r2.id FROM album_revisions r2 WHERE r2.album_id = r.album_id
               ORDER BY r2.id DESC LIMIT 1)
FROM album_revisions r, jsonb_array_elements(r.data -> 'credits') c
WHERE c ? 'credited_as';

-- Aliases from artists' history that were removed since.
INSERT INTO spellings
SELECT r.artist_id, a, false
FROM artist_revisions r, jsonb_array_elements_text(r.data -> 'aliases') a;

-- Make listed aliases for spellings in use (unless the artist already has it listed)...
INSERT INTO artist_aliases (artist_id, alias, listed)
SELECT DISTINCT s.artist_id, s.alias, true
FROM spellings s
WHERE s.in_use
  AND NOT EXISTS (SELECT 1 FROM artist_aliases al
                  WHERE al.artist_id = s.artist_id AND al.alias = s.alias AND al.listed);

-- ...and unlisted ones for spellings only in history.
INSERT INTO artist_aliases (artist_id, alias, listed)
SELECT DISTINCT s.artist_id, s.alias, false
FROM spellings s
WHERE NOT EXISTS (SELECT 1 FROM artist_aliases al
                  WHERE al.artist_id = s.artist_id AND al.alias = s.alias);

-- The alias to use for a spelling: the listed one if there is one.
CREATE FUNCTION pg_temp.alias_for(p_artist uuid, p_alias text) RETURNS uuid
LANGUAGE sql STABLE AS $$
    SELECT id FROM artist_aliases
    WHERE artist_id = p_artist AND alias = p_alias
    ORDER BY listed DESC, id
    LIMIT 1
$$;

-- `credits` in revision data: {artist_id, role, credited_as} -> {artist_id, role, alias_id}.
CREATE FUNCTION pg_temp.convert_credits(p_credits jsonb) RETURNS jsonb
LANGUAGE sql STABLE AS $$
    SELECT jsonb_agg(
        CASE WHEN c ? 'credited_as'
             THEN (c - 'credited_as') || jsonb_build_object(
                      'alias_id', pg_temp.alias_for((c ->> 'artist_id')::uuid, c ->> 'credited_as'))
             ELSE c
        END ORDER BY ord)
    FROM jsonb_array_elements(p_credits) WITH ORDINALITY AS e(c, ord)
$$;

UPDATE track_revisions
SET data = jsonb_set(data, '{credits}', pg_temp.convert_credits(data -> 'credits'))
WHERE kind = 'internal' AND data ? 'credits';

UPDATE album_revisions
SET data = jsonb_set(data, '{credits}', pg_temp.convert_credits(data -> 'credits'))
WHERE data ? 'credits';

-- `aliases` in artist revision data: ["name"] -> [{"id": ..., "name": "name"}].
UPDATE artist_revisions r
SET data = jsonb_set(r.data, '{aliases}', (
    SELECT jsonb_agg(jsonb_build_object('id', pg_temp.alias_for(r.artist_id, a), 'name', a)
                     ORDER BY ord)
    FROM jsonb_array_elements_text(r.data -> 'aliases') WITH ORDINALITY AS e(a, ord)))
WHERE r.data ? 'aliases';

-- Artists whose listed aliases now differ from their latest revision (a credited
-- spelling became an alias) get a revision recording them.
INSERT INTO artist_revisions (artist_id, source, reason, data)
SELECT a.id, 'edit', 'credited spellings became aliases',
       jsonb_build_object('name', a.name)
       || CASE WHEN a.disambiguation <> ''
               THEN jsonb_build_object('disambiguation', a.disambiguation) ELSE '{}' END
       || jsonb_build_object('aliases', (
              SELECT jsonb_agg(jsonb_build_object('id', al.id, 'name', al.alias) ORDER BY al.id)
              FROM artist_aliases al WHERE al.artist_id = a.id AND al.listed))
FROM artists a
WHERE EXISTS (SELECT 1 FROM artist_aliases al WHERE al.artist_id = a.id AND al.listed)
  AND (SELECT count(*) FROM artist_aliases al WHERE al.artist_id = a.id AND al.listed)
      <> COALESCE((SELECT jsonb_array_length(r.data -> 'aliases') FROM artist_revisions r
                   WHERE r.artist_id = a.id ORDER BY r.id DESC LIMIT 1), 0);

-- ---- Current credits: credited_as -> alias_id ----

ALTER TABLE track_credits ADD COLUMN alias_id uuid;
UPDATE track_credits SET alias_id = pg_temp.alias_for(artist_id, credited_as)
WHERE credited_as IS NOT NULL;
ALTER TABLE track_credits
    DROP COLUMN credited_as,
    -- The alias must belong to the credited artist. DEFERRABLE: merging moves aliases to
    -- another artist and changes the credits in the same transaction.
    ADD FOREIGN KEY (artist_id, alias_id) REFERENCES artist_aliases (artist_id, id)
        DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE album_credits ADD COLUMN alias_id uuid;
UPDATE album_credits SET alias_id = pg_temp.alias_for(artist_id, credited_as)
WHERE credited_as IS NOT NULL;
ALTER TABLE album_credits
    DROP COLUMN credited_as,
    ADD FOREIGN KEY (artist_id, alias_id) REFERENCES artist_aliases (artist_id, id)
        DEFERRABLE INITIALLY DEFERRED;

CREATE INDEX track_credits_alias_idx ON track_credits (alias_id) WHERE alias_id IS NOT NULL;
CREATE INDEX album_credits_alias_idx ON album_credits (alias_id) WHERE alias_id IS NOT NULL;

-- ============================================================================
-- Derived artist text: alias name if credited under one
-- ============================================================================

CREATE OR REPLACE FUNCTION artist_display_of(p_track_id uuid) RETURNS text
LANGUAGE sql STABLE
AS $$
    WITH credited AS (
        SELECT c.role, c.position, COALESCE(al.alias, a.name) AS name
        FROM track_credits c
        JOIN artists a ON a.id = c.artist_id
        LEFT JOIN artist_aliases al ON al.id = c.alias_id
        WHERE c.track_id = p_track_id
    )
    SELECT btrim(
        COALESCE((SELECT string_agg(name, ', ' ORDER BY position)
                  FROM credited WHERE role = 'primary'), '')
        || COALESCE(' feat. ' || (SELECT string_agg(name, ', ' ORDER BY position)
                                  FROM credited WHERE role = 'featured'), ''))
$$;

UPDATE tracks SET artist_display = artist_display_of(id);

-- ============================================================================
-- Finding revisions that credit an artist (for merging)
-- ============================================================================

-- `data @> '{"credits": [{"artist_id": "..."}]}'` uses these.
CREATE INDEX track_revisions_data_idx ON track_revisions USING gin (data jsonb_path_ops);
CREATE INDEX album_revisions_data_idx ON album_revisions USING gin (data jsonb_path_ops);
