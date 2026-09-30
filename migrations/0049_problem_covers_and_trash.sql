-- Files that didn't import (`ingest_problems`):
--
-- * The cover images sent with one are kept too (they were lost before: only the rest of
--   the description was), as blobs named in the description as `sha256:<hash>`, and
--   listed here so a purge knows they're in use.
-- * An entry resolved by hand (not imported) whose audio nothing else uses shows in the
--   trash, from when it was resolved: purging it deletes its audio and covers (marked
--   `audio_purged_at`), restoring it opens the entry again. Before, that audio stayed in
--   the blob store for good.

ALTER TABLE ingest_problems
    ADD COLUMN cover_blobs     text[] NOT NULL DEFAULT '{}',
    ADD COLUMN audio_purged_at timestamptz;

ALTER TABLE purge_log DROP CONSTRAINT purge_log_kind_check;
ALTER TABLE purge_log ADD CONSTRAINT purge_log_kind_check CHECK (kind IN ('cover', 'source', 'file', 'problem'));
