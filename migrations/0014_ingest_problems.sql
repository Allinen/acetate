-- Files that didn't import: refused (missing tags, not audio, bad description...) or failed
-- (an error on our side). Listed on the Review page instead of only in logs, with the
-- audio kept, so a big import's leftovers can be gone through later.
--
-- One open entry per file (sha256): importing the same file again, e.g. re-running a
-- folder, updates it (`attempts`) instead of adding another. When the file later imports
-- fine, its entry is resolved by itself, pointing at the new track.

CREATE TABLE ingest_problems (
    id                uuid PRIMARY KEY DEFAULT uuidv7(),
    kind              text NOT NULL CHECK (kind IN ('skipped', 'failed')),
    -- The audio is kept in the blob store under this hash (see `db::trash::purge`, which
    -- doesn't delete a blob an open entry still uses).
    sha256            text NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    size_bytes        bigint NOT NULL,
    uploaded_filename text NOT NULL,
    -- Where the CLI found it (NULL for uploads).
    path              text,
    reason            text NOT NULL,
    -- The origin it was given ({platform, source_id?, captured_at?, data}).
    origin            jsonb,
    -- The raw tags, as stored for files (`{"tags": [...]}`), if they could be read.
    embedded_tags     jsonb,
    attempts          integer NOT NULL DEFAULT 1,
    created_at        timestamptz NOT NULL DEFAULT now(),
    created_by        uuid DEFAULT current_app_user() REFERENCES users (id) ON DELETE SET NULL,
    last_attempt_at   timestamptz NOT NULL DEFAULT now(),
    resolved_at       timestamptz,
    resolved_by       uuid REFERENCES users (id) ON DELETE SET NULL,
    resolution        text NOT NULL DEFAULT '',
    -- The track the file became, when it imported fine later.
    track_id          uuid REFERENCES tracks (id)
);

CREATE UNIQUE INDEX ingest_problems_open_key ON ingest_problems (sha256) WHERE resolved_at IS NULL;

-- Whoever uploaded the file sees its entry; admins see all (also the CLI's, which has no
-- user). The file never became a track, so there's nothing else to check.
ALTER TABLE ingest_problems ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON ingest_problems TO acetate_app
    USING (created_by = (SELECT app_viewer()) OR (SELECT app_sees_all()));
REVOKE DELETE ON ingest_problems FROM acetate_app;
