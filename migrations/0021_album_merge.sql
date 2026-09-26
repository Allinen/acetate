-- Merging albums (see `merge_album` in src/db/revisions.rs): the tracks, covers, sources,
-- annotations and shares of one album move to another, and the merged album stays as a row
-- redirecting to it (`merged_into`), like a merged artist or track, so old revisions,
-- links and ids still resolve.

ALTER TABLE albums
    ADD COLUMN merged_into uuid REFERENCES albums (id),
    ADD CONSTRAINT albums_not_merged_into_self CHECK (merged_into <> id);

CREATE INDEX albums_merged_into_idx ON albums (merged_into) WHERE merged_into IS NOT NULL;

-- An album without tracks is visible to everyone (it reveals nothing), but a merged album
-- has no tracks left and would reveal its title. It's visible when the album it was
-- merged into is: that album's tracks are checked through album_tracks' own policy, since
-- a policy on albums can't query albums itself (that would recurse).
DROP POLICY visible ON albums;
CREATE POLICY visible ON albums TO acetate_app USING (
    CASE WHEN (SELECT current_app_share()) IS NULL
         THEN EXISTS (SELECT 1 FROM album_tracks at WHERE at.album_id = albums.id)
              OR (merged_into IS NULL AND NOT album_has_tracks(id))
              OR (merged_into IS NOT NULL
                  AND (EXISTS (SELECT 1 FROM album_tracks at WHERE at.album_id = albums.merged_into)
                       OR NOT album_has_tracks(merged_into)))
         ELSE id = (SELECT app_share_album())
    END
);
