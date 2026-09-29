-- A file's origin (`files.origin_snapshot_id`) can be pointed at another snapshot of its
-- track's sources: a download recorded as coming from the wrong place, say, or a source
-- added later that says it better. Files stay otherwise unchangeable: the app may update
-- this one column, and every change is kept here (`db::sources::set_file_origin`).

CREATE TABLE file_origin_changes (
    id              uuid PRIMARY KEY DEFAULT uuidv7(),
    file_id         uuid NOT NULL REFERENCES files (id),
    -- NULL: the snapshot was purged since (with its source, from the trash).
    old_snapshot_id uuid REFERENCES source_snapshots (id) ON DELETE SET NULL,
    new_snapshot_id uuid REFERENCES source_snapshots (id) ON DELETE SET NULL,
    changed_at      timestamptz NOT NULL DEFAULT now(),
    changed_by      uuid REFERENCES users (id) ON DELETE SET NULL DEFAULT current_app_user(),
    reason          text NOT NULL DEFAULT ''
);

CREATE INDEX file_origin_changes_file_idx ON file_origin_changes (file_id, id DESC);

-- Visible with the file; never rewritten.
ALTER TABLE file_origin_changes ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON file_origin_changes TO acetate_app
    USING (EXISTS (SELECT 1 FROM files f WHERE f.id = file_id));
REVOKE UPDATE, DELETE ON file_origin_changes FROM acetate_app;

-- One more column that may change (besides the trash's, migration 0009). Which files:
-- those the user can see, as the trash's update policy already says.
GRANT UPDATE (origin_snapshot_id) ON files TO acetate_app;
