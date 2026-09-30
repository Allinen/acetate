-- A track turned into an extra (0042) sent every client the whole catalog: it leaves
-- everyone's sight, and a delta never says something is gone that some couldn't see (it
-- would tell them it existed). But a track that wasn't restricted was seen by everyone:
-- saying it's gone tells nobody anything new. So now it's logged as gone (`track_gone`),
-- and so is an album it was the last track of (`album_gone`: that album was seen by
-- everyone too, through it); its other albums and its artists as changed. Only a
-- restricted one, or an album left with restricted tracks only (hidden from some), still
-- sends everything. Made a track again, it comes back as an ordinary change (seen by those
-- who see it).

ALTER TABLE library_changes DROP CONSTRAINT library_changes_kind_check;
ALTER TABLE library_changes ADD CONSTRAINT library_changes_kind_check
    CHECK (kind IN ('track', 'album', 'artist', 'annotation', 'grouping', 'extra', 'track_gone', 'album_gone', 'everything'));

CREATE OR REPLACE FUNCTION tracks_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
DECLARE
    a record;
BEGIN
    IF TG_OP = 'UPDATE' AND OLD.is_restricted IS DISTINCT FROM NEW.is_restricted THEN
        PERFORM log_change('everything', NULL);
    ELSIF TG_OP = 'UPDATE' AND OLD.became_extra_id IS DISTINCT FROM NEW.became_extra_id THEN
        IF NEW.is_restricted THEN
            PERFORM log_change('everything', NULL);
        ELSIF NEW.became_extra_id IS NOT NULL THEN
            PERFORM log_change('track_gone', NEW.id);
            FOR a IN
                SELECT at.album_id,
                       NOT EXISTS (SELECT 1 FROM album_tracks x JOIN tracks t ON t.id = x.track_id
                                   WHERE x.album_id = at.album_id AND t.id <> NEW.id AND t.became_extra_id IS NULL) AS emptied,
                       EXISTS (SELECT 1 FROM album_tracks x JOIN tracks t ON t.id = x.track_id
                               WHERE x.album_id = at.album_id AND t.id <> NEW.id AND t.became_extra_id IS NULL
                                 AND NOT t.is_restricted) AS open_left
                FROM album_tracks at WHERE at.track_id = NEW.id
            LOOP
                IF a.emptied THEN
                    PERFORM log_change('album_gone', a.album_id);
                ELSIF NOT a.open_left THEN
                    PERFORM log_change('everything', NULL);
                ELSE
                    PERFORM log_change('album', a.album_id);
                END IF;
            END LOOP;
        ELSE
            PERFORM log_change('album', at.album_id) FROM album_tracks at WHERE at.track_id = NEW.id;
        END IF;
        PERFORM log_change('artist', c.artist_id) FROM track_credits c WHERE c.track_id = NEW.id;
    END IF;
    PERFORM log_change('track', COALESCE(NEW.id, OLD.id));
    RETURN NULL;
END
$$;
