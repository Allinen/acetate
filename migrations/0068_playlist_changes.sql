-- Playlists in the catalog's deltas: a delta used to carry every playlist (and every
-- grouping) each time. Now a change to a playlist or its entries is logged (`playlist`),
-- and a delta has the list only when one changed. (A playlist's change is logged for
-- everyone: the list a user gets has their own and the public ones, and what changed in
-- someone's private one tells nothing but that the list is asked for again.)

ALTER TABLE library_changes DROP CONSTRAINT library_changes_kind_check;
ALTER TABLE library_changes ADD CONSTRAINT library_changes_kind_check
    CHECK (kind IN ('track', 'album', 'artist', 'annotation', 'grouping', 'extra', 'track_gone', 'album_gone', 'playlist', 'everything'));

CREATE FUNCTION playlists_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF TG_TABLE_NAME = 'playlists' THEN
        PERFORM log_change('playlist', COALESCE(NEW.id, OLD.id));
    ELSE
        PERFORM log_change('playlist', COALESCE(NEW.playlist_id, OLD.playlist_id));
    END IF;
    RETURN NULL;
END
$$;
CREATE TRIGGER playlists_log_change AFTER INSERT OR UPDATE OR DELETE ON playlists
    FOR EACH ROW EXECUTE FUNCTION playlists_log_change();
CREATE TRIGGER playlist_entries_log_change AFTER INSERT OR UPDATE OR DELETE ON playlist_entries
    FOR EACH ROW EXECUTE FUNCTION playlists_log_change();
