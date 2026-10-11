-- A track or album has each image once among its covers (not counting the trash). Of the
-- same image several times, the first (by position) stays and the rest go to the trash
-- (the covers' undo; purging them later keeps the image, which the first still uses).
UPDATE covers c SET trashed_at = now()
WHERE c.trashed_at IS NULL
  AND EXISTS (
      SELECT 1 FROM covers k
      WHERE k.trashed_at IS NULL AND k.blob_sha256 = c.blob_sha256
        AND k.track_id IS NOT DISTINCT FROM c.track_id AND k.album_id IS NOT DISTINCT FROM c.album_id
        AND (k.position, k.id) < (c.position, c.id)
  );

CREATE UNIQUE INDEX covers_track_image_idx ON covers (track_id, blob_sha256)
    WHERE track_id IS NOT NULL AND trashed_at IS NULL;
CREATE UNIQUE INDEX covers_album_image_idx ON covers (album_id, blob_sha256)
    WHERE album_id IS NOT NULL AND trashed_at IS NULL;
