-- The catalog lists each track's source platforms (`source:youtube` in the clients), so
-- a track's sources changing (added, moved, trashed) changes the track.
CREATE FUNCTION sources_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF TG_OP <> 'INSERT' AND OLD.track_id IS NOT NULL THEN
        PERFORM log_change('track', OLD.track_id);
    END IF;
    IF TG_OP <> 'DELETE' AND NEW.track_id IS NOT NULL
       AND (TG_OP = 'INSERT' OR NEW.track_id IS DISTINCT FROM OLD.track_id
            OR NEW.platform IS DISTINCT FROM OLD.platform OR NEW.trashed_at IS DISTINCT FROM OLD.trashed_at) THEN
        PERFORM log_change('track', NEW.track_id);
    END IF;
    RETURN NULL;
END
$$;
CREATE TRIGGER sources_log_change AFTER INSERT OR UPDATE OR DELETE ON sources
    FOR EACH ROW EXECUTE FUNCTION sources_log_change();
