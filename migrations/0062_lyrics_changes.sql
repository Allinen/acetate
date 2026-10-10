-- Lyrics are part of the catalog (`lyrics`, see `db::library::LYRIC_COLUMNS`), so clients
-- have them offline: a lyric added, changed or removed is a change to its track, for
-- deltas. Clients reading the new format fetch the whole catalog once anyway (its format
-- changed), so nothing older needs logging here.

CREATE FUNCTION lyrics_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF TG_OP <> 'INSERT' THEN
        PERFORM log_change('track', OLD.track_id);
    END IF;
    IF TG_OP <> 'DELETE' AND (TG_OP = 'INSERT' OR NEW.track_id IS DISTINCT FROM OLD.track_id) THEN
        PERFORM log_change('track', NEW.track_id);
    END IF;
    RETURN NULL;
END
$$;
CREATE TRIGGER lyrics_log_change AFTER INSERT OR UPDATE OR DELETE ON lyrics
    FOR EACH ROW EXECUTE FUNCTION lyrics_log_change();
