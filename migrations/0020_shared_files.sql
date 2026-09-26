-- Several tracks can play the same file: e.g. two versions of a song that turned out to be
-- the same audio keep their own ids and metadata but store the bytes once
-- (`POST /api/tracks/{id}/use-file`). The file a track gave up becomes one of its previous
-- files (track_file_history), like after an upgrade, so it can be taken back or trashed.

ALTER TABLE tracks DROP CONSTRAINT tracks_file_id_key;
-- Still looked up by file (which tracks play this?), just not unique anymore.
CREATE INDEX tracks_file_idx ON tracks (file_id);
