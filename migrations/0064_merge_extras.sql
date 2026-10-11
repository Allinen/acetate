-- Extras merged into another (the same file twice, or several extras that are one thing):
-- the files and links move to the extra kept, the old one is taken off and goes to the
-- trash (`trashed_at`), where an admin can purge it after the grace period, or anyone can
-- take it back out (which undoes the merge: its files and links go back). A purged extra
-- stays as a tombstone (`purged_at`), with its log. `merged_into` lets an old link find
-- the extra it became. See `db::extras::merge`.

ALTER TABLE extras
    ADD COLUMN merged_into uuid REFERENCES extras (id),
    ADD COLUMN trashed_at  timestamptz,
    ADD COLUMN trashed_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    ADD COLUMN purged_at   timestamptz,
    ADD COLUMN purged_by   uuid REFERENCES users (id) ON DELETE SET NULL,
    ADD CONSTRAINT extras_merged_into_other CHECK (merged_into IS NULL OR merged_into <> id),
    ADD CONSTRAINT extras_purged_after_trashed CHECK (purged_at IS NULL OR trashed_at IS NOT NULL);
CREATE INDEX extras_merged_into_idx ON extras (merged_into) WHERE merged_into IS NOT NULL;
CREATE INDEX extras_trashed_idx ON extras (trashed_at) WHERE trashed_at IS NOT NULL;

ALTER TABLE purge_log DROP CONSTRAINT purge_log_kind_check;
ALTER TABLE purge_log ADD CONSTRAINT purge_log_kind_check CHECK (kind IN ('cover', 'source', 'file', 'problem', 'extra'));
