-- Who sees a track (restricted or not, its groups) or an extra changing no longer sends
-- every client the whole catalog: it's logged as a change of the track (and its albums
-- and artists) or the extra, and a delta lists what the user can't see anymore as
-- removed (see `api::library`). Someone who could see it knew it anyway (and could tell
-- it's gone by trying to play it); the protection is restricting before sharing. A delta
-- may so name the id of a track some never saw (nothing else about it). The whole catalog
-- is still sent when a user's own groups or a group's permissions change (what they see
-- changes across the library).

CREATE OR REPLACE FUNCTION tracks_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND (OLD.is_restricted IS DISTINCT FROM NEW.is_restricted
                             OR OLD.became_extra_id IS DISTINCT FROM NEW.became_extra_id) THEN
        PERFORM log_change('album', at.album_id) FROM album_tracks at WHERE at.track_id = NEW.id;
        PERFORM log_change('artist', c.artist_id) FROM track_credits c WHERE c.track_id = NEW.id;
    END IF;
    PERFORM log_change('track', COALESCE(NEW.id, OLD.id));
    RETURN NULL;
END
$$;

-- A track's groups: that track (and its albums).
CREATE FUNCTION track_access_log_row() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
DECLARE
    v_track uuid := COALESCE(NEW.track_id, OLD.track_id);
BEGIN
    PERFORM log_change('track', v_track);
    PERFORM log_change('album', at.album_id) FROM album_tracks at WHERE at.track_id = v_track;
    RETURN NULL;
END
$$;
DROP TRIGGER track_access_log_change ON track_access;
CREATE TRIGGER track_access_log_change AFTER INSERT OR UPDATE OR DELETE ON track_access
    FOR EACH ROW EXECUTE FUNCTION track_access_log_row();
CREATE TRIGGER track_access_truncated AFTER TRUNCATE ON track_access
    FOR EACH STATEMENT EXECUTE FUNCTION log_everything();

-- Extras: restricted or not, and their groups, are a change of the extra.
CREATE OR REPLACE FUNCTION extras_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    PERFORM log_change('extra', COALESCE(NEW.id, OLD.id));
    RETURN NULL;
END
$$;
CREATE OR REPLACE FUNCTION extra_access_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF TG_OP = 'TRUNCATE' THEN
        PERFORM log_change('everything', NULL);
    ELSE
        PERFORM log_change('extra', COALESCE(NEW.extra_id, OLD.extra_id));
    END IF;
    RETURN NULL;
END
$$;
