-- Phase 3: row-level security. Restricted tracks are filtered by Postgres itself, so a
-- query that forgets a WHERE clause can't leak them, and nothing (no count, no artist, no
-- album) may reveal that something hidden exists.
--
-- Postgres doesn't apply RLS to a table's owner or to superusers, so the server works as
-- `acetate_app`: its connections run `SET ROLE acetate_app` (see `serve` in main.rs).
-- Migrations and the CLI stay the owner, i.e. the system, which sees everything.
--
-- Who sees a track:
--   * not restricted: everyone;
--   * restricted: admins, and members of a group in `track_access`;
--   * in a share transaction: only the share's tracks, and only those its creator can see.
-- Everything else follows from the tracks: a track's variants, revisions, credits, file,
-- covers... are visible when the track is; an album when one of its tracks is; an artist
-- when it's credited on a visible track or album.
--
-- Writes use the same conditions: you can only change rows you can see.

-- ============================================================================
-- The server's role
-- ============================================================================

-- Roles belong to the whole Postgres server, not to one database, so it may already
-- exist (e.g. from another database on the same server).
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'acetate_app') THEN
        CREATE ROLE acetate_app NOLOGIN;
    END IF;
END $$;

-- Lets the connecting user (the owner) switch to it.
GRANT acetate_app TO CURRENT_USER;

GRANT USAGE ON SCHEMA public TO acetate_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO acetate_app;
-- Tables added by later migrations get the same.
ALTER DEFAULT PRIVILEGES IN SCHEMA public
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO acetate_app;

-- Nothing the server does needs these, so it can't do them even by mistake.
REVOKE ALL ON _sqlx_migrations FROM acetate_app;
-- Append-only: files are immutable, history is never rewritten.
REVOKE UPDATE, DELETE ON files, track_file_history,
    track_revisions, artist_revisions, album_revisions FROM acetate_app;

-- ============================================================================
-- Who is looking (helpers for the policies)
-- ============================================================================
--
-- SECURITY DEFINER: these run as the owner, so they can read `track_access`, `shares`,
-- `album_tracks`... without going through the policies themselves (which would loop).
-- They only answer yes/no or return ids of the share's own target, never hidden data.
-- `SET search_path` is the usual precaution for SECURITY DEFINER functions.
-- Policies call them as `(SELECT f())`, which Postgres evaluates once per query.

-- The share this transaction serves, if it's still active.
CREATE FUNCTION app_share() RETURNS shares
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public, pg_temp
AS $$
    SELECT * FROM shares
    WHERE id = current_app_share()
      AND revoked_at IS NULL
      AND (expires_at IS NULL OR expires_at > now())
$$;

-- Whose eyes we look through: the logged-in user, or for a share, the one who made it.
CREATE FUNCTION app_viewer() RETURNS uuid
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public, pg_temp
AS $$
    SELECT CASE WHEN current_app_share() IS NULL THEN current_app_user()
                ELSE (app_share()).created_by
           END
$$;

-- Admins see every track.
CREATE FUNCTION app_sees_all() RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public, pg_temp
AS $$
    SELECT EXISTS (
        SELECT 1 FROM user_groups ug
        JOIN group_permissions gp ON gp.group_id = ug.group_id
        WHERE ug.user_id = app_viewer() AND gp.permission = 'admin')
$$;

CREATE FUNCTION app_groups() RETURNS uuid[]
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public, pg_temp
AS $$
    SELECT COALESCE(array_agg(group_id), '{}') FROM user_groups WHERE user_id = app_viewer()
$$;

-- Whether a restricted track is open to one of the viewer's groups. Only called for
-- restricted tracks (few), so once per row is fine.
CREATE FUNCTION restricted_track_visible(p_track_id uuid) RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public, pg_temp
AS $$
    SELECT EXISTS (
        SELECT 1 FROM track_access
        WHERE track_id = p_track_id AND group_id = ANY (app_groups()))
$$;

-- The tracks the share opens up (before checking what its creator can see).
-- NULL outside a share transaction, or when the share isn't active.
CREATE FUNCTION app_share_tracks() RETURNS uuid[]
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public, pg_temp
AS $$
    SELECT CASE WHEN s.track_id IS NOT NULL THEN ARRAY[s.track_id]
                ELSE ARRAY(SELECT track_id FROM album_tracks WHERE album_id = s.album_id)
           END
    FROM app_share() s
    WHERE s.id IS NOT NULL
$$;

CREATE FUNCTION app_share_album() RETURNS uuid
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public, pg_temp
AS $$ SELECT (app_share()).album_id $$;

-- "Has no tracks at all" / "is credited nowhere", counting hidden ones too: an album or
-- artist with nothing attached reveals nothing, so everyone can see it (e.g. one that was
-- just created).
CREATE FUNCTION album_has_tracks(p_album_id uuid) RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public, pg_temp
AS $$ SELECT EXISTS (SELECT 1 FROM album_tracks WHERE album_id = p_album_id) $$;

CREATE FUNCTION artist_is_credited(p_artist_id uuid) RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public, pg_temp
AS $$
    SELECT EXISTS (SELECT 1 FROM track_credits WHERE artist_id = p_artist_id)
        OR EXISTS (SELECT 1 FROM album_credits WHERE artist_id = p_artist_id)
$$;

-- ============================================================================
-- Policies
-- ============================================================================
--
-- `FOR ALL` with only `USING`: the same condition is also the check for inserted and
-- updated rows. Policies apply to `acetate_app` only; the owner isn't affected anyway.

ALTER TABLE tracks ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON tracks TO acetate_app USING (
    (NOT is_restricted OR (SELECT app_sees_all()) OR restricted_track_visible(id))
    AND ((SELECT current_app_share()) IS NULL OR id IN (SELECT unnest(app_share_tracks())))
);

-- ---- Rows belonging to one track: visible when the track is ----
-- The `tracks` inside these goes through the policy above, so the rule lives in one place.

ALTER TABLE track_variants ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON track_variants TO acetate_app
    USING (EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id));

ALTER TABLE track_revisions ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON track_revisions TO acetate_app
    USING (EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id));

ALTER TABLE track_credits ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON track_credits TO acetate_app
    USING (EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id));

ALTER TABLE album_tracks ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON album_tracks TO acetate_app
    USING (EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id));

ALTER TABLE track_file_history ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON track_file_history TO acetate_app
    USING (EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id));

ALTER TABLE lyrics ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON lyrics TO acetate_app
    USING (EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id));

ALTER TABLE review_flags ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON review_flags TO acetate_app
    USING (EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id));

ALTER TABLE track_access ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON track_access TO acetate_app
    USING (EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id));

-- A relation shows only when both of its tracks do.
ALTER TABLE track_relations ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON track_relations TO acetate_app USING (
    EXISTS (SELECT 1 FROM tracks t WHERE t.id = from_track_id)
    AND EXISTS (SELECT 1 FROM tracks t WHERE t.id = to_track_id)
);

-- A file: the current or an earlier file of a visible track. Anyone may add one (a new
-- file row reveals nothing, and during an upload it's written before the track points at
-- it); files are never changed or deleted (see the REVOKE above).
ALTER TABLE files ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON files FOR SELECT TO acetate_app USING (
    EXISTS (SELECT 1 FROM tracks t WHERE t.file_id = files.id)
    OR EXISTS (SELECT 1 FROM track_file_history h WHERE h.old_file_id = files.id)
);
CREATE POLICY add ON files FOR INSERT TO acetate_app WITH CHECK (true);

-- ---- Albums: visible with one of their tracks ----

ALTER TABLE albums ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON albums TO acetate_app USING (
    CASE WHEN (SELECT current_app_share()) IS NULL
         THEN EXISTS (SELECT 1 FROM album_tracks at WHERE at.album_id = albums.id)
              OR NOT album_has_tracks(id)
         ELSE id = (SELECT app_share_album())
    END
);

ALTER TABLE album_credits ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON album_credits TO acetate_app
    USING (EXISTS (SELECT 1 FROM albums al WHERE al.id = album_id));

ALTER TABLE album_revisions ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON album_revisions TO acetate_app
    USING (EXISTS (SELECT 1 FROM albums al WHERE al.id = album_id));

-- ---- Artists: visible when credited on something visible ----

ALTER TABLE artists ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON artists TO acetate_app USING (
    EXISTS (SELECT 1 FROM track_credits c WHERE c.artist_id = artists.id)
    OR EXISTS (SELECT 1 FROM album_credits c WHERE c.artist_id = artists.id)
    OR ((SELECT current_app_share()) IS NULL AND NOT artist_is_credited(id))
);

ALTER TABLE artist_aliases ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON artist_aliases TO acetate_app
    USING (EXISTS (SELECT 1 FROM artists a WHERE a.id = artist_id));

ALTER TABLE artist_revisions ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON artist_revisions TO acetate_app
    USING (EXISTS (SELECT 1 FROM artists a WHERE a.id = artist_id));

-- ---- Things owned by a track, an album or an artist ----

ALTER TABLE covers ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON covers TO acetate_app USING (
    (track_id IS NOT NULL AND EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id))
    OR (album_id IS NOT NULL AND EXISTS (SELECT 1 FROM albums al WHERE al.id = album_id))
);

ALTER TABLE sources ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON sources TO acetate_app USING (
    (track_id IS NOT NULL AND EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id))
    OR (album_id IS NOT NULL AND EXISTS (SELECT 1 FROM albums al WHERE al.id = album_id))
    OR (artist_id IS NOT NULL AND EXISTS (SELECT 1 FROM artists a WHERE a.id = artist_id))
);

ALTER TABLE source_snapshots ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON source_snapshots TO acetate_app
    USING (EXISTS (SELECT 1 FROM sources s WHERE s.id = source_id));

-- ============================================================================
-- Changes to access: who restricted what, when
-- ============================================================================

-- Append-only, like revisions. `groups` = [{id, name}] as they were at the time.
CREATE TABLE track_access_log (
    id         uuid PRIMARY KEY DEFAULT uuidv7(),
    track_id   uuid NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
    changed_at timestamptz NOT NULL DEFAULT now(),
    changed_by uuid DEFAULT current_app_user() REFERENCES users (id) ON DELETE SET NULL,
    restricted boolean NOT NULL,
    groups     jsonb NOT NULL DEFAULT '[]',
    reason     text NOT NULL DEFAULT ''
);

CREATE INDEX track_access_log_track_idx ON track_access_log (track_id, id DESC);

ALTER TABLE track_access_log ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON track_access_log TO acetate_app
    USING (EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id));
REVOKE UPDATE, DELETE ON track_access_log FROM acetate_app;

-- ============================================================================
-- Checks that must see hidden rows
-- ============================================================================

-- Both halves of a relation must exist, including halves the editor can't see.
ALTER FUNCTION check_relation_pair(uuid, text, uuid)
    SECURITY DEFINER SET search_path = public, pg_temp;

-- ============================================================================
-- Keeping search fast
-- ============================================================================
--
-- To make sure a policy runs before anything that could leak what it hides, Postgres
-- only uses an index for a condition if the condition's function is LEAKPROOF (can't
-- reveal anything about the rows it looks at, e.g. through an error). Otherwise it checks
-- the policy on every row first, which means reading the whole table.
--
-- The search operators (`<%` = word_similarity_op, `ILIKE` on text = texticlike) aren't
-- marked LEAKPROOF, although they are in practice: they return true/false for any
-- input and never raise an error that depends on the row. Marking them needs a superuser;
-- without one, search still works, it just reads every row (the server warns at startup).
DO $$
BEGIN
    IF (SELECT rolsuper FROM pg_roles WHERE rolname = current_user) THEN
        ALTER FUNCTION word_similarity_op(text, text) LEAKPROOF;
        ALTER FUNCTION texticlike(text, text) LEAKPROOF;
    ELSE
        RAISE WARNING 'not a superuser: search operators not marked LEAKPROOF, search will be slow';
    END IF;
END $$;

UPDATE permissions SET description = 'Restrict tracks and choose which groups can see them'
WHERE permission = 'manage_access';
