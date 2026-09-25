-- Undoing a track merge.
--
-- A merge records exactly what it moved and changed (`moved`, see `MergeRecord` in
-- src/db/revisions.rs), so it can be reversed as long as nothing has happened since that
-- depends on it (see `merge_undo_blockers`). Merges recorded before this have no
-- `moved` and can't be undone.

ALTER TABLE track_merges
    ADD COLUMN moved     jsonb,
    ADD COLUMN undone_at timestamptz,
    ADD COLUMN undone_by uuid REFERENCES users (id) ON DELETE SET NULL;
