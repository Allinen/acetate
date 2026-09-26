-- Every album merge, with a record of exactly what it moved (`moved`, see
-- `AlbumMergeRecord` in src/db/revisions.rs), so it can be undone. The undo shows
-- beforehand what goes back and what doesn't (`undo_album_merge`). Merges made before
-- this (0021) have no record and can't be undone.
CREATE TABLE album_merges (
    id            uuid PRIMARY KEY DEFAULT uuidv7(),
    into_album_id uuid NOT NULL REFERENCES albums (id),
    from_album_id uuid NOT NULL REFERENCES albums (id),
    merged_at     timestamptz NOT NULL DEFAULT now(),
    merged_by     uuid DEFAULT current_app_user() REFERENCES users (id) ON DELETE SET NULL,
    reason        text NOT NULL DEFAULT '',
    moved         jsonb NOT NULL,
    undone_at     timestamptz,
    undone_by     uuid REFERENCES users (id) ON DELETE SET NULL
);

CREATE INDEX album_merges_into_idx ON album_merges (into_album_id);

-- Visible with the album merged into. Written by the merge and the undo, as the system.
ALTER TABLE album_merges ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON album_merges TO acetate_app
    USING (EXISTS (SELECT 1 FROM albums al WHERE al.id = into_album_id));
REVOKE UPDATE, DELETE ON album_merges FROM acetate_app;
