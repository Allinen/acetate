-- Phase 3b: lyrics history, and a trash for covers, sources and old audio files.
--
-- Nothing is deleted directly. Covers, sources and audio files that are no longer wanted
-- go to the trash (`trashed_at`), where they're hidden but can be taken back out. After
-- a grace period (7 days, `TRASH_GRACE` in src/db/trash.rs) an admin can purge them:
-- that's the only way anything is deleted, and every purge is logged in `purge_log`.
-- Lyrics aren't trashed at all: they're revisioned (like metadata) and small.

-- ============================================================================
-- Lyrics history
-- ============================================================================

-- Like the other revision tables: append-only, full snapshots, restore = new revision.
-- data = {body, is_official, language}. The `lyrics` row holds the current state
-- (written only by src/db/revisions.rs); `is_synced` is derived from the body.
CREATE TABLE lyrics_revisions (
    id         uuid PRIMARY KEY DEFAULT uuidv7(),
    lyrics_id  uuid NOT NULL REFERENCES lyrics (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    author_id  uuid DEFAULT current_app_user() REFERENCES users (id) ON DELETE SET NULL,
    source     text NOT NULL CHECK (source IN ('import', 'edit', 'restore')),
    reason     text NOT NULL DEFAULT '',
    data       jsonb NOT NULL
);

CREATE INDEX lyrics_revisions_lyrics_idx ON lyrics_revisions (lyrics_id, id DESC);

-- Revision 0 for the lyrics that exist already.
INSERT INTO lyrics_revisions (lyrics_id, created_at, author_id, source, reason, data)
SELECT id, created_at, created_by,
       CASE WHEN source = 'embedded' THEN 'import' ELSE 'edit' END,
       CASE WHEN source = 'embedded' THEN 'imported_from_file' ELSE '' END,
       jsonb_build_object('body', body)
       || CASE WHEN is_official THEN '{"is_official": true}'::jsonb ELSE '{}' END
       || CASE WHEN language IS NOT NULL THEN jsonb_build_object('language', language) ELSE '{}' END
FROM lyrics;

ALTER TABLE lyrics_revisions ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON lyrics_revisions TO acetate_app
    USING (EXISTS (SELECT 1 FROM lyrics l WHERE l.id = lyrics_id));
REVOKE UPDATE, DELETE ON lyrics_revisions FROM acetate_app;

-- ============================================================================
-- Trash
-- ============================================================================

ALTER TABLE covers
    ADD COLUMN trashed_at timestamptz,
    ADD COLUMN trashed_by uuid REFERENCES users (id) ON DELETE SET NULL;
ALTER TABLE sources
    ADD COLUMN trashed_at timestamptz,
    ADD COLUMN trashed_by uuid REFERENCES users (id) ON DELETE SET NULL;

-- Files: trashed, then purged. A purged file keeps its row (a tombstone: its tags,
-- origin and place in the track's history stay), only its bytes are deleted.
ALTER TABLE files
    ADD COLUMN trashed_at timestamptz,
    ADD COLUMN trashed_by uuid REFERENCES users (id) ON DELETE SET NULL,
    ADD COLUMN purged_at  timestamptz,
    ADD COLUMN purged_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    ADD CONSTRAINT files_purged_after_trashed CHECK (purged_at IS NULL OR trashed_at IS NOT NULL);

-- The same bytes can be stored again after a purge (as a new file); among files that
-- still have their bytes, a hash stays unique. (Same name as before: ingest looks for it
-- to recognize two uploads of one file racing each other.)
ALTER TABLE files DROP CONSTRAINT files_sha256_key;
CREATE UNIQUE INDEX files_sha256_key ON files (sha256) WHERE purged_at IS NULL;

-- Files are otherwise immutable for the server (migration 0008 revoked UPDATE); these
-- four columns are the exception. Column-level: nothing else about a file can change.
GRANT UPDATE (trashed_at, trashed_by, purged_at, purged_by) ON files TO acetate_app;
CREATE POLICY trash ON files FOR UPDATE TO acetate_app USING (
    EXISTS (SELECT 1 FROM tracks t WHERE t.file_id = files.id)
    OR EXISTS (SELECT 1 FROM track_file_history h WHERE h.old_file_id = files.id)
);

-- For the trash list, and for "is this blob still used?" before deleting one.
CREATE INDEX covers_trashed_idx ON covers (trashed_at) WHERE trashed_at IS NOT NULL;
CREATE INDEX sources_trashed_idx ON sources (trashed_at) WHERE trashed_at IS NOT NULL;
CREATE INDEX files_trashed_idx ON files (trashed_at) WHERE trashed_at IS NOT NULL AND purged_at IS NULL;
CREATE INDEX covers_blob_idx ON covers (blob_sha256);

-- ============================================================================
-- Purge log
-- ============================================================================

-- What was purged, by whom, when. `description` keeps what the item was (title,
-- filename, sha256...), since the item itself is gone (or, for files, only a tombstone).
CREATE TABLE purge_log (
    id          uuid PRIMARY KEY DEFAULT uuidv7(),
    purged_at   timestamptz NOT NULL DEFAULT now(),
    purged_by   uuid DEFAULT current_app_user() REFERENCES users (id) ON DELETE SET NULL,
    kind        text NOT NULL CHECK (kind IN ('cover', 'source', 'file')),
    item_id     uuid NOT NULL,
    description jsonb NOT NULL DEFAULT '{}',
    bytes_freed bigint NOT NULL DEFAULT 0
);

-- Admins only: it describes things from hidden tracks too.
ALTER TABLE purge_log ENABLE ROW LEVEL SECURITY;
CREATE POLICY admins ON purge_log TO acetate_app USING ((SELECT app_sees_all()));
REVOKE UPDATE, DELETE ON purge_log FROM acetate_app;
