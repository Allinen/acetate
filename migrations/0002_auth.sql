-- Phase 1: logins, and knowing who did what.

-- ============================================================================
-- Who is doing this
-- ============================================================================

-- Every transaction the server runs on behalf of a user starts with
--   SELECT set_config('app.user_id', '<user id>', true)
-- (see `db::begin`). `true` = only for this transaction, so a pooled connection never
-- carries one request's user into the next. The CLI runs without a user: NULL = system.
--
-- Column defaults below use this to record who created a row, and the row-level
-- security policies (Phase 3) will use it to decide what that user can see.
CREATE FUNCTION current_app_user() RETURNS uuid
LANGUAGE sql STABLE
-- Outside such a transaction the setting is missing (NULL) or '' (after one ended on
-- this connection); both mean "no user".
AS $$ SELECT NULLIF(current_setting('app.user_id', true), '')::uuid $$;

-- Existing "who" columns: filled in automatically from now on.
ALTER TABLE files              ALTER COLUMN uploaded_by SET DEFAULT current_app_user();
ALTER TABLE track_revisions    ALTER COLUMN author_id   SET DEFAULT current_app_user();
ALTER TABLE track_file_history ALTER COLUMN replaced_by SET DEFAULT current_app_user();

-- And for things added outside of revisions.
ALTER TABLE sources
    ADD COLUMN created_by uuid DEFAULT current_app_user() REFERENCES users (id) ON DELETE SET NULL;
ALTER TABLE source_snapshots
    ADD COLUMN created_by uuid DEFAULT current_app_user() REFERENCES users (id) ON DELETE SET NULL;
ALTER TABLE lyrics
    ADD COLUMN created_by uuid DEFAULT current_app_user() REFERENCES users (id) ON DELETE SET NULL;
ALTER TABLE covers
    ADD COLUMN created_by uuid DEFAULT current_app_user() REFERENCES users (id) ON DELETE SET NULL;

-- ============================================================================
-- Sessions
-- ============================================================================

-- A login. The client holds a random token (cookie for the web UI, `Authorization:
-- Bearer` for other clients); only its hash is stored, so a leaked database doesn't
-- leak working logins. Deleting a row logs that device out.
CREATE TABLE sessions (
    id           uuid PRIMARY KEY DEFAULT uuidv7(),
    user_id      uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_sha256 text NOT NULL UNIQUE CHECK (token_sha256 ~ '^[0-9a-f]{64}$'),
    user_agent   text,
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_used_at timestamptz NOT NULL DEFAULT now(),
    -- Pushed forward while the session is used; unused sessions run out.
    expires_at   timestamptz NOT NULL
);

CREATE INDEX sessions_user_id_idx ON sessions (user_id);

-- ============================================================================
-- Permissions
-- ============================================================================

INSERT INTO permissions (permission, description) VALUES
    ('browse', 'See the library: list, search and view tracks, albums, artists and covers');

INSERT INTO group_permissions (group_id, permission)
SELECT id, 'browse' FROM groups WHERE name = 'user';

-- Guests (visitors who aren't logged in) have no permissions, and that's fixed in code
-- (`api::auth::authenticate`), not a setting: a library open to the internet is too big a
-- risk to be one admin click away. So the seeded `guest` group goes.
DELETE FROM groups WHERE name = 'guest';

-- The `admin` permission implies every other one (the server checks it that way), so the
-- admin group doesn't need each permission granted, including ones added later.
DELETE FROM group_permissions gp
USING groups g
WHERE gp.group_id = g.id AND g.name = 'admin' AND gp.permission <> 'admin';
