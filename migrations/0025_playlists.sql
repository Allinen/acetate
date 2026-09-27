-- Playlists: a user's ordered lists of tracks.
--
-- * Private (only the owner) or public (every logged-in user can see it; only the owner
--   changes it). Share links for guests come later (shares.playlist_id).
-- * The same track can be in a playlist more than once: each entry is its own row.
-- * Tracks someone can't see stay invisible in a playlist they can see: the entry is
--   left out for them (the tracks policy decides), and nothing tells them it's there.
-- * Deleting puts a playlist in the trash (restorable). Every change to the entries is
--   logged, with what's needed to undo it (`playlist_log`).
-- * When a track is merged into another, its entries follow (recorded in the merge, so
--   undoing it puts them back, see `revisions::merge_tracks`).

CREATE TABLE playlists (
    id          uuid PRIMARY KEY DEFAULT uuidv7(),
    owner_id    uuid NOT NULL DEFAULT current_app_user() REFERENCES users (id) ON DELETE CASCADE,
    name        text NOT NULL CHECK (btrim(name) <> ''),
    description text NOT NULL DEFAULT '',
    public      boolean NOT NULL DEFAULT false,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    trashed_at  timestamptz,
    -- Where an imported playlist came from, e.g. "navidrome:<id>": importing again finds
    -- it instead of making another.
    imported_from text,
    UNIQUE (owner_id, imported_from)
);

CREATE INDEX playlists_owner_idx ON playlists (owner_id);
CREATE INDEX playlists_public_idx ON playlists (id) WHERE public AND trashed_at IS NULL;

CREATE TABLE playlist_entries (
    id          uuid PRIMARY KEY DEFAULT uuidv7(),
    playlist_id uuid NOT NULL REFERENCES playlists (id) ON DELETE CASCADE,
    track_id    uuid NOT NULL REFERENCES tracks (id),
    -- The order: 0, 1, 2... (rewritten on reorder; playlists are small).
    position    integer NOT NULL CHECK (position >= 0),
    added_at    timestamptz NOT NULL DEFAULT now(),
    added_by    uuid DEFAULT current_app_user() REFERENCES users (id) ON DELETE SET NULL,
    UNIQUE (playlist_id, position) DEFERRABLE INITIALLY DEFERRED
);

CREATE INDEX playlist_entries_track_idx ON playlist_entries (track_id);

-- What was done to a playlist's entries, newest last, each with what undoing it needs:
-- `detail` holds the entries added or removed (id, track, position) or the order before.
CREATE TABLE playlist_log (
    id          uuid PRIMARY KEY DEFAULT uuidv7(),
    playlist_id uuid NOT NULL REFERENCES playlists (id) ON DELETE CASCADE,
    at          timestamptz NOT NULL DEFAULT now(),
    by_user     uuid DEFAULT current_app_user() REFERENCES users (id) ON DELETE SET NULL,
    action      text NOT NULL CHECK (action IN ('add', 'remove', 'reorder', 'undo', 'import')),
    detail      jsonb NOT NULL DEFAULT '{}',
    undone_at   timestamptz
);

CREATE INDEX playlist_log_playlist_idx ON playlist_log (playlist_id, id);

-- ---- Row-level security ----
--
-- Seeing: the owner always; others only public ones that aren't in the trash. Guests
-- (no user) see none. Changing: the owner only.

ALTER TABLE playlists ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON playlists FOR SELECT TO acetate_app USING (
    owner_id = (SELECT app_viewer())
    OR (public AND trashed_at IS NULL AND (SELECT current_app_user()) IS NOT NULL)
);
CREATE POLICY own_insert ON playlists FOR INSERT TO acetate_app
    WITH CHECK (owner_id = (SELECT current_app_user()));
CREATE POLICY own_update ON playlists FOR UPDATE TO acetate_app
    USING (owner_id = (SELECT current_app_user()))
    WITH CHECK (owner_id = (SELECT current_app_user()));
-- Playlists are trashed, not deleted, by the app (an admin's purge would run as the
-- system).
REVOKE DELETE ON playlists FROM acetate_app;

-- An entry is visible when its playlist is and its track is (a hidden track's entry
-- doesn't exist for whoever can't see the track).
ALTER TABLE playlist_entries ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON playlist_entries FOR SELECT TO acetate_app USING (
    EXISTS (SELECT 1 FROM playlists p WHERE p.id = playlist_id)
    AND EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id)
);
CREATE POLICY own_insert ON playlist_entries FOR INSERT TO acetate_app WITH CHECK (
    EXISTS (SELECT 1 FROM playlists p WHERE p.id = playlist_id AND p.owner_id = (SELECT current_app_user()))
    AND EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id)
);
CREATE POLICY own_update ON playlist_entries FOR UPDATE TO acetate_app USING (
    EXISTS (SELECT 1 FROM playlists p WHERE p.id = playlist_id AND p.owner_id = (SELECT current_app_user()))
);
CREATE POLICY own_delete ON playlist_entries FOR DELETE TO acetate_app USING (
    EXISTS (SELECT 1 FROM playlists p WHERE p.id = playlist_id AND p.owner_id = (SELECT current_app_user()))
);

-- The log: the owner's.
ALTER TABLE playlist_log ENABLE ROW LEVEL SECURITY;
CREATE POLICY own ON playlist_log TO acetate_app USING (
    EXISTS (SELECT 1 FROM playlists p WHERE p.id = playlist_id AND p.owner_id = (SELECT current_app_user()))
);
REVOKE DELETE ON playlist_log FROM acetate_app;
