-- Phase 3c: merging duplicate tracks (the same song, stored twice).
--
-- The duplicate is merged *into* the track that's kept: its file, variants, covers,
-- lyrics, sources, albums and relations move over (see `merge_tracks` in
-- src/db/revisions.rs). What's left of it is a row that redirects to the kept track
-- (`merged_into`), like a merged artist, so old links, shares and history still resolve.

ALTER TABLE tracks
    ADD COLUMN merged_into uuid REFERENCES tracks (id),
    ADD CONSTRAINT tracks_not_merged_into_self CHECK (merged_into <> id);

CREATE INDEX tracks_merged_into_idx ON tracks (merged_into) WHERE merged_into IS NOT NULL;

-- A merged track hands its file over to the kept track (as current or previous file), and
-- keeps none: `file_id` is unique. Every other track has a file.
ALTER TABLE tracks
    ALTER COLUMN file_id DROP NOT NULL,
    ADD CONSTRAINT tracks_file_unless_merged CHECK (file_id IS NOT NULL OR merged_into IS NOT NULL);

-- Moving a variant to another track moves its revisions along. (The server can't change
-- revisions itself, see 0008; this is the foreign key keeping them consistent.)
ALTER TABLE track_revisions
    DROP CONSTRAINT track_revisions_track_id_variant_id_fkey,
    ADD CONSTRAINT track_revisions_track_id_variant_id_fkey
        FOREIGN KEY (track_id, variant_id) REFERENCES track_variants (track_id, id)
        ON DELETE CASCADE ON UPDATE CASCADE;

-- Every merge, with which side won each field: what happened, by whom, and enough to
-- undo it by hand if it ever has to be.
CREATE TABLE track_merges (
    id            uuid PRIMARY KEY DEFAULT uuidv7(),
    into_track_id uuid NOT NULL REFERENCES tracks (id),
    from_track_id uuid NOT NULL REFERENCES tracks (id),
    merged_at     timestamptz NOT NULL DEFAULT now(),
    merged_by     uuid DEFAULT current_app_user() REFERENCES users (id) ON DELETE SET NULL,
    picks         jsonb NOT NULL DEFAULT '{}',
    reason        text NOT NULL DEFAULT ''
);

CREATE INDEX track_merges_into_idx ON track_merges (into_track_id);

ALTER TABLE track_merges ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON track_merges TO acetate_app
    USING (EXISTS (SELECT 1 FROM tracks t WHERE t.id = into_track_id));
REVOKE UPDATE, DELETE ON track_merges FROM acetate_app;
