-- Extras linked to a grouping (a collective's, an era's…), as to a track, album or artist.
-- An artist turned into a collective has its extras move there (`db::groupings`: the
-- link's other side changes, logged in `extra_log`).

ALTER TABLE extra_links ADD COLUMN grouping_id uuid REFERENCES groupings (id);
ALTER TABLE extra_links DROP CONSTRAINT extra_links_check;
ALTER TABLE extra_links ADD CONSTRAINT extra_links_check CHECK (num_nonnulls(track_id, album_id, artist_id, grouping_id) = 1);
DROP INDEX extra_links_once_idx;
CREATE UNIQUE INDEX extra_links_once_idx ON extra_links (extra_id, COALESCE(track_id, album_id, artist_id, grouping_id));
CREATE INDEX extra_links_grouping_idx ON extra_links (grouping_id) WHERE grouping_id IS NOT NULL;

-- Both sides must be visible (a grouping: to everyone logged in).
DROP POLICY visible ON extra_links;
CREATE POLICY visible ON extra_links TO acetate_app USING (
    EXISTS (SELECT 1 FROM extras e WHERE e.id = extra_id)
    AND ((track_id IS NOT NULL AND EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id))
         OR (album_id IS NOT NULL AND EXISTS (SELECT 1 FROM albums al WHERE al.id = album_id))
         OR (artist_id IS NOT NULL AND EXISTS (SELECT 1 FROM artists a WHERE a.id = artist_id))
         OR (grouping_id IS NOT NULL AND EXISTS (SELECT 1 FROM groupings g WHERE g.id = grouping_id)))
);
