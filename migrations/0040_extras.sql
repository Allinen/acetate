-- Extras: audio (and other files) that isn't a track: vocals, instrumentals, stems, samples,
-- clips, project files. Kept like tracks' files (stored once, by hash), with light
-- metadata: a name, a kind (free text; clients suggest the usual ones), the files, and
-- links to the tracks, albums and artists they belong to (many to many). They stay out of
-- the library's lists, albums and shuffle; they're found on those pages and by search.
--
-- Restricted like tracks: `is_restricted`, and the groups in `extra_access` who see it
-- (admins see everything; shares don't reach extras). A link shows only when both of its
-- sides do. Nothing is deleted: an extra or one of its files is taken off (hidden, kept,
-- brought back); every change is in `extra_log`. See `db::extras`.

CREATE TABLE extras (
    id            uuid PRIMARY KEY DEFAULT uuidv7(),
    name          text NOT NULL CHECK (name <> ''),
    kind          text NOT NULL DEFAULT 'other' CHECK (kind <> ''),
    is_restricted boolean NOT NULL DEFAULT false,
    -- Taken off (kept).
    hidden_at     timestamptz,
    created_at    timestamptz NOT NULL DEFAULT now(),
    created_by    uuid REFERENCES users (id) ON DELETE SET NULL DEFAULT current_app_user()
);

CREATE TABLE extra_access (
    extra_id uuid NOT NULL REFERENCES extras (id),
    group_id uuid NOT NULL REFERENCES groups (id) ON DELETE CASCADE,
    PRIMARY KEY (extra_id, group_id)
);

-- Its files: bytes in the blob store (shared with anything else that has the same bytes).
CREATE TABLE extra_files (
    id          uuid PRIMARY KEY DEFAULT uuidv7(),
    extra_id    uuid NOT NULL REFERENCES extras (id),
    sha256      text NOT NULL REFERENCES blobs (sha256),
    -- The name it was added with.
    name        text NOT NULL,
    size_bytes  bigint NOT NULL,
    -- For audio (else NULL).
    container   text,
    codec       text,
    sample_rate integer,
    channels    smallint,
    bit_depth   smallint,
    duration_ms bigint,
    hidden_at   timestamptz,
    added_at    timestamptz NOT NULL DEFAULT now(),
    added_by    uuid REFERENCES users (id) ON DELETE SET NULL DEFAULT current_app_user()
);
CREATE INDEX extra_files_extra_idx ON extra_files (extra_id);
CREATE INDEX extra_files_sha256_idx ON extra_files (sha256);

CREATE TABLE extra_links (
    id         uuid PRIMARY KEY DEFAULT uuidv7(),
    extra_id   uuid NOT NULL REFERENCES extras (id),
    track_id   uuid REFERENCES tracks (id),
    album_id   uuid REFERENCES albums (id),
    artist_id  uuid REFERENCES artists (id),
    CHECK (num_nonnulls(track_id, album_id, artist_id) = 1),
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by uuid REFERENCES users (id) ON DELETE SET NULL DEFAULT current_app_user()
);
CREATE UNIQUE INDEX extra_links_once_idx ON extra_links (extra_id, COALESCE(track_id, album_id, artist_id));
CREATE INDEX extra_links_track_idx ON extra_links (track_id) WHERE track_id IS NOT NULL;
CREATE INDEX extra_links_album_idx ON extra_links (album_id) WHERE album_id IS NOT NULL;
CREATE INDEX extra_links_artist_idx ON extra_links (artist_id) WHERE artist_id IS NOT NULL;

-- Everything done to an extra: made, renamed, restricted, a file added or taken off, a
-- link made or undone, taken off, brought back.
CREATE TABLE extra_log (
    id         uuid PRIMARY KEY DEFAULT uuidv7(),
    extra_id   uuid NOT NULL REFERENCES extras (id),
    what       text NOT NULL,
    details    jsonb NOT NULL DEFAULT '{}',
    reason     text NOT NULL DEFAULT '',
    at         timestamptz NOT NULL DEFAULT now(),
    by_user    uuid REFERENCES users (id) ON DELETE SET NULL DEFAULT current_app_user()
);
CREATE INDEX extra_log_extra_idx ON extra_log (extra_id, id DESC);

-- ---- Who sees what ----

CREATE FUNCTION restricted_extra_visible(p_extra_id uuid) RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public, pg_temp
AS $$
    SELECT EXISTS (SELECT 1 FROM extra_access WHERE extra_id = p_extra_id AND group_id = ANY (app_groups()))
$$;

ALTER TABLE extras ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON extras TO acetate_app USING (
    (NOT is_restricted OR (SELECT app_sees_all()) OR restricted_extra_visible(id))
    AND (SELECT current_app_share()) IS NULL
);

ALTER TABLE extra_access ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON extra_access TO acetate_app
    USING (EXISTS (SELECT 1 FROM extras e WHERE e.id = extra_id));

ALTER TABLE extra_files ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON extra_files TO acetate_app
    USING (EXISTS (SELECT 1 FROM extras e WHERE e.id = extra_id));

-- Both sides must be visible.
ALTER TABLE extra_links ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON extra_links TO acetate_app USING (
    EXISTS (SELECT 1 FROM extras e WHERE e.id = extra_id)
    AND ((track_id IS NOT NULL AND EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id))
         OR (album_id IS NOT NULL AND EXISTS (SELECT 1 FROM albums al WHERE al.id = album_id))
         OR (artist_id IS NOT NULL AND EXISTS (SELECT 1 FROM artists a WHERE a.id = artist_id)))
);

ALTER TABLE extra_log ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON extra_log TO acetate_app
    USING (EXISTS (SELECT 1 FROM extras e WHERE e.id = extra_id));

-- Nothing is deleted but links (undone, and logged); the log is never rewritten.
REVOKE DELETE ON extras, extra_files FROM acetate_app;
REVOKE UPDATE, DELETE ON extra_log FROM acetate_app;
-- The blobs table is the system's (`db::blobs`); extras' files only point at it.
