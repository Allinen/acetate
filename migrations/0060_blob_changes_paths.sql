-- Mirrors also get each blob's path where it was imported from (`files.imported_from`, or
-- a file that didn't import's `path`): a mirror finds a file it already has on its own
-- disk by that path and its size, without hashing every file it has. So a path filled in
-- later (an import finding the file stored already, migration 0024) is a change too.

DROP TRIGGER files_log_blob_change ON files;
CREATE TRIGGER files_log_blob_change AFTER INSERT OR UPDATE OF trashed_at, purged_at, imported_from OR DELETE ON files
    FOR EACH ROW EXECUTE FUNCTION files_log_blob_change();

DROP TRIGGER ingest_problems_log_blob_change ON ingest_problems;
CREATE TRIGGER ingest_problems_log_blob_change
    AFTER INSERT OR UPDATE OF sha256, cover_blobs, resolved_at, track_id, audio_purged_at, path OR DELETE ON ingest_problems
    FOR EACH ROW EXECUTE FUNCTION ingest_problems_log_blob_change();
