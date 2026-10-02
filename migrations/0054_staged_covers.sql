-- Cover images uploaded before they're covers: the desktop editor stages an image (pasted,
-- or chosen from a file) as soon as it's given, and the track edit that uses it
-- (`POST /api/tracks/edit`, see `db::track_edits`) makes it a cover of each of its tracks,
-- only when applied. Its previews refer to it by hash and send no image.
--
-- An image is only ever used by whoever staged it: someone else knowing its hash gets
-- nothing (it isn't a cover they can see). The blob is stored like any cover's; one never
-- used stays in the store as "other" (nothing refers to it), like an upload abandoned
-- halfway.

CREATE TABLE staged_covers (
    user_id     uuid NOT NULL DEFAULT current_app_user() REFERENCES users (id) ON DELETE CASCADE,
    blob_sha256 text NOT NULL CHECK (blob_sha256 ~ '^[0-9a-f]{64}$'),
    mime        text,
    width       integer,
    height      integer,
    staged_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, blob_sha256)
);

ALTER TABLE staged_covers ENABLE ROW LEVEL SECURITY;
CREATE POLICY own ON staged_covers TO acetate_app
    USING (user_id = (SELECT current_app_user()))
    WITH CHECK (user_id = (SELECT current_app_user()));
