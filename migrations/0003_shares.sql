-- Sharing: a link that opens one track or album (later: playlist) for whoever has it,
-- logged in or not. A share is the only way a guest (not logged in) sees anything.

INSERT INTO permissions (permission, description) VALUES
    ('share', 'Create share links for tracks and albums');

CREATE TABLE shares (
    id           uuid PRIMARY KEY DEFAULT uuidv7(),
    -- Like sessions: the link holds a random token and only its hash is stored, so a
    -- leaked database doesn't leak working links (to e.g. unreleased tracks).
    token_sha256 text NOT NULL UNIQUE CHECK (token_sha256 ~ '^[0-9a-f]{64}$'),
    -- What's shared. Exactly one; `playlist_id` joins once playlists exist.
    track_id     uuid REFERENCES tracks (id) ON DELETE CASCADE,
    album_id     uuid REFERENCES albums (id) ON DELETE CASCADE,
    CHECK (num_nonnulls(track_id, album_id) = 1),
    -- Deleting the user ends their shares.
    created_by   uuid NOT NULL DEFAULT current_app_user() REFERENCES users (id) ON DELETE CASCADE,
    created_at   timestamptz NOT NULL DEFAULT now(),
    expires_at   timestamptz,  -- NULL = doesn't expire
    revoked_at   timestamptz
);

CREATE INDEX shares_created_by_idx ON shares (created_by);
CREATE INDEX shares_track_id_idx ON shares (track_id) WHERE track_id IS NOT NULL;
CREATE INDEX shares_album_id_idx ON shares (album_id) WHERE album_id IS NOT NULL;

-- The share a transaction is serving (set by `db::begin_share`), like `app.user_id` for
-- users. Row-level security (Phase 3) will let a share see exactly the rows it opens up,
-- restricted tracks included: that's how a share gets around RLS, and only for its target.
CREATE FUNCTION current_app_share() RETURNS uuid
LANGUAGE sql STABLE
AS $$ SELECT NULLIF(current_setting('app.share_id', true), '')::uuid $$;
