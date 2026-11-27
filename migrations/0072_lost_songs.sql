-- Lost songs: songs known to exist (a deleted upload, a snippet someone posted, a title in
-- a tracklist) whose audio the library doesn't have. Each is an extra of kind `lost_song`:
-- its files are what's left of it (cover art, screenshots, a snippet), its links are its
-- artists, and `lost` holds what is known about it (see `db::extras::Lost`). Unlike other
-- extras it can have sources (shaped like a track's), so looking up a platform id finds a
-- song that is known to be lost.
--
-- When a track and a live lost song come to share a source (the same platform id), the
-- track gets a `lost_song_found` flag. Resolving it as found gives the track the lost
-- song's sources and takes the lost song off: a song the library has isn't lost.

INSERT INTO vocabulary (list, word, label, description, locked, position)
SELECT 'extra_kind', 'lost_song', 'Lost song', 'A song whose audio the library doesn''t have', true, COALESCE(max(position), 0) + 1
FROM vocabulary WHERE list = 'extra_kind';

INSERT INTO vocabulary (list, word, label, description, locked, position, data)
SELECT 'review_kind', 'lost_song_found', 'Lost song found', 'This track may be a song that was listed as lost', true,
       COALESCE(max(position), 0) + 1, jsonb_build_object('automatic', false, 'checked', true)
FROM vocabulary WHERE list = 'review_kind';

ALTER TABLE extras
    ADD COLUMN lost jsonb,
    ADD CONSTRAINT extras_lost_iff_lost_song CHECK ((kind = 'lost_song') = (lost IS NOT NULL));
CREATE INDEX extras_lost_status_idx ON extras ((lost ->> 'status')) WHERE lost IS NOT NULL;

-- ---- Sources: a lost song can have them ----
ALTER TABLE sources ADD COLUMN extra_id uuid REFERENCES extras (id);
ALTER TABLE sources DROP CONSTRAINT sources_check;
ALTER TABLE sources ADD CONSTRAINT sources_check CHECK (num_nonnulls(track_id, album_id, artist_id, grouping_id, extra_id) = 1);
DROP INDEX sources_external_id_key;
CREATE UNIQUE INDEX sources_external_id_key
    ON sources ((COALESCE(track_id, album_id, artist_id, grouping_id, extra_id)), platform, external_id)
    WHERE external_id IS NOT NULL;
CREATE INDEX sources_extra_idx ON sources (extra_id) WHERE extra_id IS NOT NULL;

DROP POLICY visible ON sources;
CREATE POLICY visible ON sources TO acetate_app USING (
    (track_id IS NOT NULL AND EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id))
    OR (album_id IS NOT NULL AND EXISTS (SELECT 1 FROM albums al WHERE al.id = album_id))
    OR (artist_id IS NOT NULL AND EXISTS (SELECT 1 FROM artists a WHERE a.id = artist_id))
    OR (grouping_id IS NOT NULL AND EXISTS (SELECT 1 FROM groupings g WHERE g.id = grouping_id))
    OR (extra_id IS NOT NULL AND EXISTS (SELECT 1 FROM extras e WHERE e.id = extra_id))
);

-- One open flag per track and lost song.
CREATE UNIQUE INDEX review_flags_open_lost_song ON review_flags (track_id, (related ->> 'extra_id'))
    WHERE kind = 'lost_song_found' AND resolved_at IS NULL;

-- A source given to a track or to a lost song (made, moved, or taken out of the trash):
-- if the other side has the same platform id, the track is flagged. Checked at commit, so
-- a lost song taken off in the same transaction (resolved as found) raises nothing. Runs
-- as the owner: the flag is raised even when whoever added the source can't see the lost
-- song (the flag is seen with the track; the lost song stays as hidden as it was). A track
-- that was flagged for a lost song once (even if resolved as not it) isn't flagged again.
CREATE FUNCTION sources_lost_song_found() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
DECLARE
    s sources;
BEGIN
    SELECT * INTO s FROM sources WHERE id = NEW.id;
    IF s.id IS NULL OR s.external_id IS NULL OR s.trashed_at IS NOT NULL
       OR (s.track_id IS NULL AND s.extra_id IS NULL) THEN
        RETURN NULL;
    END IF;
    INSERT INTO review_flags (kind, track_id, related)
    SELECT 'lost_song_found', t.id,
           jsonb_build_object('extra_id', e.id, 'platform', s.platform, 'external_id', s.external_id)
    FROM sources other
    JOIN tracks t ON t.id = COALESCE(s.track_id, other.track_id)
    JOIN extras e ON e.id = COALESCE(s.extra_id, other.extra_id)
    WHERE other.platform = s.platform AND other.external_id = s.external_id
      AND other.trashed_at IS NULL AND other.id <> s.id
      AND (CASE WHEN s.track_id IS NOT NULL THEN other.extra_id IS NOT NULL ELSE other.track_id IS NOT NULL END)
      AND t.merged_into IS NULL
      AND e.kind = 'lost_song' AND e.hidden_at IS NULL AND e.merged_into IS NULL
      AND NOT EXISTS (SELECT 1 FROM review_flags f
                      WHERE f.kind = 'lost_song_found' AND f.track_id = t.id AND f.related ->> 'extra_id' = e.id::text)
    ON CONFLICT DO NOTHING;
    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER sources_lost_song_found AFTER INSERT OR UPDATE OF track_id, extra_id, trashed_at ON sources
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION sources_lost_song_found();
