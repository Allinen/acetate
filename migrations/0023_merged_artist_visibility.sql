-- A merged artist has no credits left, so it counted as "credited nowhere", which
-- everyone may see: the name of an artist only credited on hidden tracks leaked once it
-- was merged. Now a merged artist is visible when the artist it was merged into is, like a
-- merged album (0021). That artist's credits are checked through the credit tables' own
-- policies, since a policy on artists can't query artists itself (that would recurse).
DROP POLICY visible ON artists;
CREATE POLICY visible ON artists TO acetate_app USING (
    EXISTS (SELECT 1 FROM track_credits c WHERE c.artist_id = artists.id)
    OR EXISTS (SELECT 1 FROM album_credits c WHERE c.artist_id = artists.id)
    OR ((SELECT current_app_share()) IS NULL AND merged_into IS NULL AND NOT artist_is_credited(id))
    OR (merged_into IS NOT NULL
        AND (EXISTS (SELECT 1 FROM track_credits c WHERE c.artist_id = artists.merged_into)
             OR EXISTS (SELECT 1 FROM album_credits c WHERE c.artist_id = artists.merged_into)
             OR ((SELECT current_app_share()) IS NULL AND NOT artist_is_credited(merged_into))))
);
