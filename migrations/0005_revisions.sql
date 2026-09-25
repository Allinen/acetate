-- Phase 2: every change to metadata goes through a revision.
--
-- A track has two kinds of revisions, both in `track_revisions`, told apart by `kind`:
-- * internal: a variant's own metadata (title, date, genres, comment, credits). Each
--   variant has its own line of internal revisions.
-- * external: the track's place in the world, the same whatever variant is shown
--   (status, album membership, relations). One line per track.
-- Both are full snapshots, append-only. Restoring = a new revision with old data.
-- Artists and albums get their own revisions, following the same rules.
--
-- The tables holding the current state (tracks' metadata columns, track_credits,
-- album_tracks, track_relations, artists, artist_aliases, albums, album_credits) are
-- only ever written by the revision code (`db::revisions`), never directly.

-- ============================================================================
-- Track revisions: internal and external
-- ============================================================================

ALTER TABLE track_revisions ADD COLUMN kind text;
UPDATE track_revisions SET kind = 'internal';
ALTER TABLE track_revisions
    ALTER COLUMN kind SET NOT NULL,
    ADD CONSTRAINT track_revisions_kind_check CHECK (kind IN ('internal', 'external')),
    -- External revisions belong to the track, not to a variant. (The composite FK to
    -- track_variants isn't checked when variant_id is NULL.)
    ALTER COLUMN variant_id DROP NOT NULL,
    ADD CONSTRAINT track_revisions_kind_variant_check
        CHECK ((kind = 'internal') = (variant_id IS NOT NULL)),
    ALTER COLUMN author_id SET DEFAULT current_app_user();

CREATE INDEX track_revisions_external_idx ON track_revisions (track_id, id DESC)
    WHERE kind = 'external';

-- Existing internal revisions (revision 0 of each track's only variant, from import):
-- add the credits, which until now were stored only in track_credits.
UPDATE track_revisions r
SET data = r.data || jsonb_build_object('credits', c.credits)
FROM (
    SELECT track_id,
           jsonb_agg(jsonb_strip_nulls(jsonb_build_object(
               'artist_id', artist_id, 'role', role, 'credited_as', credited_as))
               ORDER BY position, artist_id) AS credits
    FROM track_credits
    GROUP BY track_id
) c
WHERE r.track_id = c.track_id AND r.kind = 'internal' AND NOT r.data ? 'credits';

-- External revision 0 for existing tracks, from their current status and albums.
-- Same author as their internal revision 0 (whoever uploaded them).
INSERT INTO track_revisions (track_id, kind, created_at, author_id, source, reason, data)
SELECT t.id, 'external', t.created_at,
       (SELECT r.author_id FROM track_revisions r
        WHERE r.track_id = t.id AND r.kind = 'internal' ORDER BY r.id LIMIT 1),
       'import', 'imported_from_file',
       jsonb_strip_nulls(jsonb_build_object(
           'status', t.status,
           'albums', (SELECT jsonb_agg(jsonb_strip_nulls(jsonb_build_object(
                          'album_id', at.album_id,
                          'disc_number', at.disc_number,
                          'track_number', at.track_number)) ORDER BY at.album_id)
                      FROM album_tracks at WHERE at.track_id = t.id),
           'relations', (SELECT jsonb_agg(jsonb_build_object(
                             'kind', tr.kind, 'track_id', tr.to_track_id)
                             ORDER BY tr.kind, tr.to_track_id)
                         FROM track_relations tr WHERE tr.from_track_id = t.id)))
FROM tracks t;

-- ============================================================================
-- Derived artist text
-- ============================================================================

-- A track's artist text, derived from its current credits: primary credits joined with
-- ", ", then " feat. " and the featured ones. Other roles (producer...) aren't shown.
-- Each credit shows the name it's credited as, else the artist's name.
--
-- This is the one definition; `tracks.artist_display` is always set from it (after
-- credits change, and after an artist is renamed), never written by hand.
CREATE FUNCTION artist_display_of(p_track_id uuid) RETURNS text
LANGUAGE sql STABLE
AS $$
    WITH credited AS (
        SELECT c.role, c.position, COALESCE(c.credited_as, a.name) AS name
        FROM track_credits c
        JOIN artists a ON a.id = c.artist_id
        WHERE c.track_id = p_track_id
    )
    SELECT btrim(
        COALESCE((SELECT string_agg(name, ', ' ORDER BY position)
                  FROM credited WHERE role = 'primary'), '')
        || COALESCE(' feat. ' || (SELECT string_agg(name, ', ' ORDER BY position)
                                  FROM credited WHERE role = 'featured'), ''))
$$;

UPDATE tracks SET artist_display = artist_display_of(id);

COMMENT ON COLUMN tracks.artist_display IS
    'Derived from the current credits by artist_display_of(); never written by hand.';
COMMENT ON TABLE track_credits IS
    'The current credits: those of the primary variant''s latest internal revision.';

-- ============================================================================
-- Artist and album revisions
-- ============================================================================

-- data: {name, disambiguation, aliases[]}. Current state in artists / artist_aliases.
CREATE TABLE artist_revisions (
    id         uuid PRIMARY KEY DEFAULT uuidv7(),
    artist_id  uuid NOT NULL REFERENCES artists (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    author_id  uuid DEFAULT current_app_user() REFERENCES users (id) ON DELETE SET NULL,
    source     text NOT NULL CHECK (source IN ('import', 'edit', 'restore')),
    reason     text NOT NULL DEFAULT '',
    data       jsonb NOT NULL CHECK (jsonb_typeof(data) = 'object')
);

CREATE INDEX artist_revisions_artist_idx ON artist_revisions (artist_id, id DESC);

-- data: {title, kind, release_date, credits[]}. Current state in albums / album_credits.
CREATE TABLE album_revisions (
    id         uuid PRIMARY KEY DEFAULT uuidv7(),
    album_id   uuid NOT NULL REFERENCES albums (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    author_id  uuid DEFAULT current_app_user() REFERENCES users (id) ON DELETE SET NULL,
    source     text NOT NULL CHECK (source IN ('import', 'edit', 'restore')),
    reason     text NOT NULL DEFAULT '',
    data       jsonb NOT NULL CHECK (jsonb_typeof(data) = 'object')
);

CREATE INDEX album_revisions_album_idx ON album_revisions (album_id, id DESC);

-- Revision 0 for existing artists and albums.
INSERT INTO artist_revisions (artist_id, created_at, source, reason, data)
SELECT a.id, a.created_at, 'import', 'imported_from_file',
       jsonb_build_object('name', a.name)
       || CASE WHEN a.disambiguation <> ''
               THEN jsonb_build_object('disambiguation', a.disambiguation)
               ELSE '{}' END
       || COALESCE((SELECT jsonb_build_object('aliases', jsonb_agg(al.alias ORDER BY al.alias))
                    FROM artist_aliases al WHERE al.artist_id = a.id
                    HAVING count(*) > 0), '{}')
FROM artists a;

INSERT INTO album_revisions (album_id, created_at, source, reason, data)
SELECT al.id, al.created_at, 'import', 'imported_from_file',
       jsonb_strip_nulls(jsonb_build_object(
           'title', al.title,
           'kind', al.kind,
           'release_date', al.release_date,
           'credits', (SELECT jsonb_agg(jsonb_strip_nulls(jsonb_build_object(
                           'artist_id', c.artist_id, 'role', c.role, 'credited_as', c.credited_as))
                           ORDER BY c.position, c.artist_id)
                       FROM album_credits c WHERE c.album_id = al.id)))
FROM albums al;

-- ============================================================================
-- Review flags
-- ============================================================================

-- Things ingest noticed that a person should look at. Nothing is changed automatically.
-- * possible_duplicate: related = {"tracks": [ids]} with similar titles by the same artist.
-- * ambiguous_artist: related = {"name": "...", "artists": [ids], "chosen": id}, when an
--   artist name from the tags matched several artists and the oldest one was used.
CREATE TABLE review_flags (
    id          uuid PRIMARY KEY DEFAULT uuidv7(),
    kind        text NOT NULL CHECK (kind IN ('possible_duplicate', 'ambiguous_artist')),
    track_id    uuid NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
    related     jsonb NOT NULL DEFAULT '{}',
    created_at  timestamptz NOT NULL DEFAULT now(),
    resolved_at timestamptz,
    resolved_by uuid REFERENCES users (id) ON DELETE SET NULL,
    resolution  text NOT NULL DEFAULT ''
);

CREATE INDEX review_flags_open_idx ON review_flags (id DESC) WHERE resolved_at IS NULL;
CREATE INDEX review_flags_track_idx ON review_flags (track_id);

-- ============================================================================
-- Permissions
-- ============================================================================

-- Credits are part of a variant's metadata now, edited together with the title.
UPDATE permissions SET description = 'Edit metadata of tracks (incl. credits, status, albums), artists and albums; creates revisions'
WHERE permission = 'edit_metadata';
UPDATE permissions SET description = 'Add/remove relations between tracks'
WHERE permission = 'edit_relations';
