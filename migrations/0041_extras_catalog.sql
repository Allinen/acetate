-- Extras in the catalog (clients search them, and list them on pages from there): a
-- change to an extra, its links or its files sends its row again. Who may see one
-- changing (restricted, its groups) sends the whole catalog, as for tracks: a delta never
-- says something is gone that was only hidden.

ALTER TABLE library_changes DROP CONSTRAINT library_changes_kind_check;
ALTER TABLE library_changes ADD CONSTRAINT library_changes_kind_check
    CHECK (kind IN ('track', 'album', 'artist', 'annotation', 'grouping', 'extra', 'everything'));

CREATE FUNCTION extras_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND OLD.is_restricted IS DISTINCT FROM NEW.is_restricted THEN
        PERFORM log_change('everything', NULL);
    END IF;
    PERFORM log_change('extra', COALESCE(NEW.id, OLD.id));
    RETURN NULL;
END
$$;
CREATE TRIGGER extras_log_change AFTER INSERT OR UPDATE OR DELETE ON extras
    FOR EACH ROW EXECUTE FUNCTION extras_log_change();

-- Its links and files are part of its row.
CREATE FUNCTION extra_parts_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    PERFORM log_change('extra', COALESCE(NEW.extra_id, OLD.extra_id));
    RETURN NULL;
END
$$;
CREATE TRIGGER extra_links_log_change AFTER INSERT OR UPDATE OR DELETE ON extra_links
    FOR EACH ROW EXECUTE FUNCTION extra_parts_log_change();
CREATE TRIGGER extra_files_log_change AFTER INSERT OR UPDATE OR DELETE ON extra_files
    FOR EACH ROW EXECUTE FUNCTION extra_parts_log_change();

-- Who sees it: per row changed (a statement that changes nothing, like clearing an
-- extra's groups when it has none, sends nothing).
CREATE FUNCTION extra_access_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    PERFORM log_change('everything', NULL);
    RETURN NULL;
END
$$;
CREATE TRIGGER extra_access_log_change AFTER INSERT OR UPDATE OR DELETE ON extra_access
    FOR EACH ROW EXECUTE FUNCTION extra_access_log_change();
CREATE TRIGGER extra_access_truncated AFTER TRUNCATE ON extra_access
    FOR EACH STATEMENT EXECUTE FUNCTION extra_access_log_change();
