-- Acetate initial schema.
--
-- Conventions:
--   * Primary keys are UUIDv7 (time-ordered, but not guessable/countable like serials,
--     so IDs never reveal that hidden rows exist).
--   * Extendable vocabularies are lookup tables with a text primary key, not PG enums.
--   * Current track metadata is denormalized onto `tracks`; history lives in `track_revisions`.

CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- ============================================================================
-- Vocabularies
-- ============================================================================

CREATE TABLE track_statuses (
    status      text PRIMARY KEY,
    description text NOT NULL DEFAULT ''
);

INSERT INTO track_statuses (status, description) VALUES
    ('released',   'Officially released'),
    ('unreleased', 'Never officially released'),
    ('demo',       'Demo version'),
    ('unfinished', 'Work in progress / unfinished'),
    ('deleted',    'Was released, later removed from official platforms');

CREATE TABLE credit_roles (
    role        text PRIMARY KEY,
    description text NOT NULL DEFAULT ''
);

INSERT INTO credit_roles (role) VALUES ('primary'), ('featured'), ('producer');

CREATE TABLE album_kinds (
    kind        text PRIMARY KEY,
    description text NOT NULL DEFAULT ''
);

INSERT INTO album_kinds (kind) VALUES
    ('album'), ('ep'), ('single'), ('compilation'), ('mixtape'), ('era'), ('project');

-- Every relation kind has an inverse. Directional kinds come in pairs
-- (remix_of <-> remix_to); symmetric kinds are their own inverse (alternate).
-- The composite FK guarantees the pairing is consistent: if X's inverse is Y,
-- then Y's inverse must be X.
CREATE TABLE relation_kinds (
    kind         text PRIMARY KEY,
    inverse_kind text NOT NULL,
    description  text NOT NULL DEFAULT '',
    UNIQUE (kind, inverse_kind),
    FOREIGN KEY (inverse_kind, kind) REFERENCES relation_kinds (kind, inverse_kind)
        DEFERRABLE INITIALLY DEFERRED
);

INSERT INTO relation_kinds (kind, inverse_kind, description) VALUES
    ('remix_of',        'remix_to',        'A is a remix of B'),
    ('remix_to',        'remix_of',        'A has B as a remix'),
    ('instrumental_of', 'instrumental_to', 'A is the instrumental of B'),
    ('instrumental_to', 'instrumental_of', 'A has B as its instrumental'),
    ('acapella_of',     'acapella_to',     'A is the acapella of B'),
    ('acapella_to',     'acapella_of',     'A has B as its acapella'),
    ('live_of',         'live_to',         'A is a live version of B'),
    ('live_to',         'live_of',         'A has B as a live version'),
    ('edit_of',         'edit_to',         'A is an edit of B'),
    ('edit_to',         'edit_of',         'A has B as an edit'),
    ('sped_up_of',      'sped_up_to',      'A is a sped-up version of B'),
    ('sped_up_to',      'sped_up_of',      'A has B as a sped-up version'),
    ('slowed_of',       'slowed_to',       'A is a slowed version of B'),
    ('slowed_to',       'slowed_of',       'A has B as a slowed version'),
    ('alternate',       'alternate',       'A and B are alternate versions of each other');

-- ============================================================================
-- Users, groups, permissions
-- ============================================================================

CREATE TABLE users (
    id            uuid PRIMARY KEY DEFAULT uuidv7(),
    username      text NOT NULL,
    password_hash text,                          -- set once auth exists (Phase 1)
    created_at    timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX users_username_key ON users (lower(username));

-- Groups are bundles of permissions, and also the unit of track access control.
CREATE TABLE groups (
    id          uuid PRIMARY KEY DEFAULT uuidv7(),
    name        text NOT NULL UNIQUE,
    description text NOT NULL DEFAULT ''
);

CREATE TABLE user_groups (
    user_id  uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    group_id uuid NOT NULL REFERENCES groups (id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, group_id)
);

CREATE INDEX user_groups_group_id_idx ON user_groups (group_id);

CREATE TABLE permissions (
    permission  text PRIMARY KEY,
    description text NOT NULL DEFAULT ''
);

INSERT INTO permissions (permission, description) VALUES
    ('admin',             'Everything, including server settings'),
    ('upload',            'Upload / import new files'),
    ('edit_metadata',     'Edit track and album metadata (creates revisions)'),
    ('view_history',      'View revision history of tracks'),
    ('restore_revision',  'Restore an older revision'),
    ('edit_relations',    'Add/remove track relations and credits'),
    ('manage_access',     'Restrict tracks and change which groups can see them'),
    ('manage_users',      'Create users, manage groups and their permissions'),
    ('download_original', 'Download the untouched original file'),
    ('replace_file',      'Replace a track''s file (e.g. upgrade to higher quality)'),
    ('edit_sources',      'Add external sources of tracks, albums and artists'),
    ('stream',            'Stream audio');

CREATE TABLE group_permissions (
    group_id   uuid NOT NULL REFERENCES groups (id) ON DELETE CASCADE,
    permission text NOT NULL REFERENCES permissions (permission),
    PRIMARY KEY (group_id, permission)
);

INSERT INTO groups (name, description) VALUES
    ('admin', 'Full access'),
    ('user',  'Regular listener'),
    ('guest', 'Stream only');

INSERT INTO group_permissions (group_id, permission)
SELECT g.id, p.permission FROM groups g CROSS JOIN permissions p WHERE g.name = 'admin';

INSERT INTO group_permissions (group_id, permission)
SELECT g.id, p FROM groups g CROSS JOIN unnest(ARRAY['stream', 'view_history']) AS p
WHERE g.name = 'user';

INSERT INTO group_permissions (group_id, permission)
SELECT g.id, 'stream' FROM groups g WHERE g.name = 'guest';

-- ============================================================================
-- Files (immutable blobs + provenance)
-- ============================================================================

CREATE TABLE files (
    id                uuid PRIMARY KEY DEFAULT uuidv7(),
    sha256            text NOT NULL UNIQUE CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    -- The file's original name: taken from its own tags (ORIGINALFILENAME, TOFN, ...)
    -- if present, otherwise the name it was uploaded with.
    original_filename text NOT NULL,
    -- The literal name of the file as it was uploaded / imported.
    uploaded_filename text NOT NULL,
    size_bytes        bigint NOT NULL,
    container         text,                     -- e.g. 'flac', 'mp3', 'ogg', 'mp4'
    codec             text,                     -- e.g. 'flac', 'mp3', 'opus', 'aac'
    bitrate           integer,                  -- kbps
    sample_rate       integer,                  -- Hz
    channels          smallint,
    bit_depth         smallint,
    duration_ms       bigint,
    embedded_tags     jsonb NOT NULL DEFAULT '{}',  -- verbatim dump of every tag in the file, never edited
    -- Where this exact file came from: a snapshot of one of its track's sources (e.g. the
    -- SoundCloud upload as it was when downloaded, or a custom "sent on Discord" source).
    -- FK added below, after source_snapshots exists.
    origin_snapshot_id uuid,
    uploaded_at       timestamptz NOT NULL DEFAULT now(),
    uploaded_by       uuid REFERENCES users (id) ON DELETE SET NULL
);

-- ============================================================================
-- Tracks, variants, revisions
-- ============================================================================

CREATE TABLE tracks (
    id                 uuid PRIMARY KEY DEFAULT uuidv7(),
    file_id            uuid NOT NULL UNIQUE REFERENCES files (id),  -- current file; history in track_file_history
    status             text NOT NULL DEFAULT 'released' REFERENCES track_statuses (status),
    is_restricted      boolean NOT NULL DEFAULT false,  -- if true, only groups in track_access can see it
    primary_variant_id uuid,                            -- FK added below (circular reference)

    -- Denormalized copy of the primary variant's latest revision.
    -- Only ever written by the revision code path.
    -- All of these are verbatim from the metadata: never parsed, trimmed or normalized.
    title              text NOT NULL,
    release_date       text,          -- e.g. '2019', '2019-05-12', '2019-05-12T11:11:11Z'
    genres             text[] NOT NULL DEFAULT '{}',

    -- Derived from release_date only for sorting/filtering by year. Never shown as "the date".
    release_year       smallint GENERATED ALWAYS AS (
                           CASE WHEN release_date ~ '^\d{4}' THEN substr(release_date, 1, 4)::smallint END
                       ) STORED,

    -- The artist tag value(s) verbatim, for display. Credits link the actual artist entities.
    artist_display     text NOT NULL DEFAULT '',

    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX tracks_release_year_idx ON tracks (release_year);
CREATE INDEX tracks_title_trgm_idx ON tracks USING gin (title gin_trgm_ops);
CREATE INDEX tracks_artist_display_trgm_idx ON tracks USING gin (artist_display gin_trgm_ops);

-- Every time a track's file is replaced (e.g. upgraded to a higher quality version),
-- the old file is kept and the change is recorded here.
CREATE TABLE track_file_history (
    id          uuid PRIMARY KEY DEFAULT uuidv7(),
    track_id    uuid NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
    old_file_id uuid NOT NULL REFERENCES files (id),
    new_file_id uuid NOT NULL REFERENCES files (id),
    replaced_at timestamptz NOT NULL DEFAULT now(),
    replaced_by uuid REFERENCES users (id) ON DELETE SET NULL,
    reason      text NOT NULL DEFAULT '',
    CHECK (old_file_id <> new_file_id)
);

CREATE INDEX track_file_history_track_idx ON track_file_history (track_id, id DESC);
CREATE INDEX track_file_history_old_file_idx ON track_file_history (old_file_id);

-- A variant is a named, intentional set of metadata for a track
-- ("File tags", "Leak title", "Spotify title"). Each variant has its own history.
CREATE TABLE track_variants (
    id         uuid PRIMARY KEY DEFAULT uuidv7(),
    track_id   uuid NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
    label      text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (track_id, id)  -- target for composite FKs below
);

CREATE INDEX track_variants_track_id_idx ON track_variants (track_id);

-- The primary variant must belong to the same track.
ALTER TABLE tracks
    ADD FOREIGN KEY (id, primary_variant_id) REFERENCES track_variants (track_id, id)
    DEFERRABLE INITIALLY DEFERRED;

-- Append-only history. A variant's current state is its newest revision.
-- Restoring an old revision inserts a new one with the old data (source = 'restore').
CREATE TABLE track_revisions (
    id         uuid PRIMARY KEY DEFAULT uuidv7(),
    track_id   uuid NOT NULL,
    variant_id uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    author_id  uuid REFERENCES users (id) ON DELETE SET NULL,  -- NULL = system
    source     text NOT NULL CHECK (source IN ('import', 'edit', 'restore')),
    reason     text NOT NULL DEFAULT '',
    is_canon   boolean NOT NULL DEFAULT false,  -- a real-world state (e.g. an old official name), not just a correction
    data       jsonb NOT NULL,
    FOREIGN KEY (track_id, variant_id) REFERENCES track_variants (track_id, id) ON DELETE CASCADE
);

-- UUIDv7 ids are time-ordered, so "newest revision of a variant" is the max id.
CREATE INDEX track_revisions_variant_idx ON track_revisions (variant_id, id DESC);
CREATE INDEX track_revisions_track_idx ON track_revisions (track_id, id DESC);

-- ============================================================================
-- Artists and credits
-- ============================================================================

CREATE TABLE artists (
    id             uuid PRIMARY KEY DEFAULT uuidv7(),
    name           text NOT NULL,
    disambiguation text NOT NULL DEFAULT '',
    created_at     timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX artists_name_lower_idx ON artists (lower(name));
CREATE INDEX artists_name_trgm_idx ON artists USING gin (name gin_trgm_ops);

CREATE TABLE artist_aliases (
    id        uuid PRIMARY KEY DEFAULT uuidv7(),
    artist_id uuid NOT NULL REFERENCES artists (id) ON DELETE CASCADE,
    alias     text NOT NULL
);

CREATE UNIQUE INDEX artist_aliases_unique_idx ON artist_aliases (artist_id, lower(alias));
CREATE INDEX artist_aliases_alias_lower_idx ON artist_aliases (lower(alias));
CREATE INDEX artist_aliases_alias_trgm_idx ON artist_aliases USING gin (alias gin_trgm_ops);

CREATE TABLE track_credits (
    track_id    uuid NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
    artist_id   uuid NOT NULL REFERENCES artists (id),
    role        text NOT NULL REFERENCES credit_roles (role),
    credited_as text,               -- the name verbatim as in the metadata, if different from artists.name
    position    smallint NOT NULL DEFAULT 0,
    PRIMARY KEY (track_id, artist_id, role)
);

CREATE INDEX track_credits_artist_idx ON track_credits (artist_id);

-- ============================================================================
-- Albums (and other groupings: eps, eras, projects...)
-- ============================================================================

CREATE TABLE albums (
    id           uuid PRIMARY KEY DEFAULT uuidv7(),
    title        text NOT NULL,
    kind         text NOT NULL DEFAULT 'album' REFERENCES album_kinds (kind),
    release_date text,  -- verbatim
    created_at   timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX albums_title_lower_idx ON albums (lower(title));
CREATE INDEX albums_title_trgm_idx ON albums USING gin (title gin_trgm_ops);

CREATE TABLE album_credits (
    album_id    uuid NOT NULL REFERENCES albums (id) ON DELETE CASCADE,
    artist_id   uuid NOT NULL REFERENCES artists (id),
    role        text NOT NULL REFERENCES credit_roles (role),
    credited_as text,
    position    smallint NOT NULL DEFAULT 0,
    PRIMARY KEY (album_id, artist_id, role)
);

CREATE INDEX album_credits_artist_idx ON album_credits (artist_id);

-- Many-to-many: the same track can be on a single, an album and a compilation.
CREATE TABLE album_tracks (
    album_id     uuid NOT NULL REFERENCES albums (id) ON DELETE CASCADE,
    track_id     uuid NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
    disc_number  smallint,
    track_number smallint,
    PRIMARY KEY (album_id, track_id)
);

CREATE INDEX album_tracks_track_idx ON album_tracks (track_id);

-- ============================================================================
-- Track relations
-- ============================================================================

-- Every relation is stored as two rows: (remix_of, A, B) and (remix_to, B, A).
-- The deferred trigger below makes sure a transaction never commits with only one half.
CREATE TABLE track_relations (
    from_track_id uuid NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
    kind          text NOT NULL REFERENCES relation_kinds (kind),
    to_track_id   uuid NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
    PRIMARY KEY (from_track_id, kind, to_track_id),
    CHECK (from_track_id <> to_track_id)
);

CREATE INDEX track_relations_to_idx ON track_relations (to_track_id);

-- Checks that (from, kind, to) and its inverse either both exist or both don't.
-- TODO(Phase 3): make SECURITY DEFINER once RLS is on, so hidden rows are still seen.
CREATE FUNCTION check_relation_pair(p_from uuid, p_kind text, p_to uuid) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    v_inverse    text;
    v_has_row    boolean;
    v_has_inverse boolean;
BEGIN
    SELECT inverse_kind INTO v_inverse FROM relation_kinds WHERE kind = p_kind;

    v_has_row := EXISTS (
        SELECT 1 FROM track_relations
        WHERE from_track_id = p_from AND kind = p_kind AND to_track_id = p_to);
    v_has_inverse := EXISTS (
        SELECT 1 FROM track_relations
        WHERE from_track_id = p_to AND kind = v_inverse AND to_track_id = p_from);

    IF v_has_row <> v_has_inverse THEN
        RAISE EXCEPTION 'track relation (%, %, %) is missing its inverse (%, %, %)',
            p_kind, p_from, p_to, v_inverse, p_to, p_from
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
END;
$$;

CREATE FUNCTION track_relations_pair_trigger() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP IN ('INSERT', 'UPDATE') THEN
        PERFORM check_relation_pair(NEW.from_track_id, NEW.kind, NEW.to_track_id);
    END IF;
    IF TG_OP IN ('DELETE', 'UPDATE') THEN
        PERFORM check_relation_pair(OLD.from_track_id, OLD.kind, OLD.to_track_id);
    END IF;
    RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER track_relations_pair_check
    AFTER INSERT OR UPDATE OR DELETE ON track_relations
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION track_relations_pair_trigger();

-- ============================================================================
-- Lyrics, covers
-- ============================================================================

CREATE TABLE lyrics (
    id          uuid PRIMARY KEY DEFAULT uuidv7(),
    track_id    uuid NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
    is_official boolean NOT NULL DEFAULT false,
    is_synced   boolean NOT NULL DEFAULT false,  -- LRC-style timestamps in body
    language    text,
    source      text NOT NULL,                   -- 'embedded', 'manual', ...
    body        text NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX lyrics_track_idx ON lyrics (track_id);

CREATE TABLE covers (
    id          uuid PRIMARY KEY DEFAULT uuidv7(),
    track_id    uuid REFERENCES tracks (id) ON DELETE CASCADE,
    album_id    uuid REFERENCES albums (id) ON DELETE CASCADE,
    blob_sha256 text NOT NULL CHECK (blob_sha256 ~ '^[0-9a-f]{64}$'),
    mime        text,
    width       integer,
    height      integer,
    position    smallint NOT NULL DEFAULT 0,  -- ordering; 0 is shown first
    source      text NOT NULL,               -- 'embedded', 'upload', ...
    created_at  timestamptz NOT NULL DEFAULT now(),
    CHECK ((track_id IS NULL) <> (album_id IS NULL))
);

CREATE INDEX covers_track_idx ON covers (track_id, position) WHERE track_id IS NOT NULL;
CREATE INDEX covers_album_idx ON covers (album_id, position) WHERE album_id IS NOT NULL;

-- ============================================================================
-- External sources (SoundCloud, NetEase, ...) of tracks, albums and artists
-- ============================================================================

-- One presence of a track/album/artist on one platform, e.g. "this song on SoundCloud,
-- song_id 555" or "this song, custom source: sent on Discord". An owner can have any
-- number of sources, also several on the same platform (e.g. two NetEase uploads).
-- The data is provided by the uploader / migration tooling; Acetate never scrapes it.
-- Platforms and their data shapes are defined in src/model/sources.rs (not in the DB),
-- so adding one is a code change in a single place.
CREATE TABLE sources (
    id          uuid PRIMARY KEY DEFAULT uuidv7(),
    track_id    uuid REFERENCES tracks (id) ON DELETE CASCADE,
    album_id    uuid REFERENCES albums (id) ON DELETE CASCADE,
    artist_id   uuid REFERENCES artists (id) ON DELETE CASCADE,
    platform    text NOT NULL,
    -- The platform's id for the owner (e.g. song_id), from the snapshot data. It identifies
    -- the source: snapshots with the same id belong to the same source. NULL for platforms
    -- without an id field (e.g. custom).
    external_id text,
    created_at  timestamptz NOT NULL DEFAULT now(),
    CHECK (num_nonnulls(track_id, album_id, artist_id) = 1)
);

-- Ids are UUIDv7 across all tables, so "the owner" is a single uuid whatever its kind.
CREATE INDEX sources_owner_idx ON sources ((COALESCE(track_id, album_id, artist_id)), platform);
CREATE UNIQUE INDEX sources_external_id_key
    ON sources ((COALESCE(track_id, album_id, artist_id)), platform, external_id)
    WHERE external_id IS NOT NULL;
-- "Which track is NetEase song 123?"
CREATE INDEX sources_platform_external_id_idx ON sources (platform, external_id)
    WHERE external_id IS NOT NULL;

-- The source's data (one platform only) as it was at some point in time. Data changes
-- (permalinks, descriptions...), so every change is a new snapshot; old ones are kept.
CREATE TABLE source_snapshots (
    id          uuid PRIMARY KEY DEFAULT uuidv7(),
    source_id   uuid NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    captured_at timestamptz NOT NULL DEFAULT now(),  -- when the data was observed on the platform
    data        jsonb NOT NULL CHECK (jsonb_typeof(data) = 'object'),
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX source_snapshots_source_idx ON source_snapshots (source_id, captured_at DESC);
-- Supports lookups on any field, e.g. data @> '{"song_permalink": "https://..."}'
-- or data @> '{"label": "discord"}'.
CREATE INDEX source_snapshots_data_idx ON source_snapshots USING gin (data jsonb_path_ops);

-- RESTRICT: a snapshot that is some file's origin can't be deleted.
ALTER TABLE files
    ADD FOREIGN KEY (origin_snapshot_id) REFERENCES source_snapshots (id) ON DELETE RESTRICT;
CREATE INDEX files_origin_snapshot_idx ON files (origin_snapshot_id);

-- ============================================================================
-- Access control
-- ============================================================================

-- Rows only exist for tracks with is_restricted = true, so this stays small
-- even with a million tracks. RLS policies using it come in Phase 3.
CREATE TABLE track_access (
    track_id uuid NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
    group_id uuid NOT NULL REFERENCES groups (id) ON DELETE CASCADE,
    PRIMARY KEY (track_id, group_id)
);

CREATE INDEX track_access_group_idx ON track_access (group_id);
