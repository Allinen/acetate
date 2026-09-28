-- What changed in the library, for clients that keep a copy of the catalog
-- (`GET /api/catalog?since=`): instead of the whole catalog again (tens of MB, seconds
-- to build), only the tracks, albums and artists that changed since their copy.
--
-- Each change is one row: what kind of thing, which one, and the transaction that
-- changed it. A client's version isn't a row id but the oldest transaction still running
-- when its catalog was read (`pg_snapshot_xmin`): a transaction that got an id earlier
-- but committed later isn't missed that way. Asking "since then" may send a change a
-- second time, which is harmless.
--
-- Written by triggers only, as the owner (SECURITY DEFINER): they see everything, and
-- the app role can't write here. What's logged is ids only; the rows sent are built by
-- the catalog's own queries, through row-level security. Changes that decide what a user
-- may see (access, groups) log `everything`: clients then fetch the whole catalog, so a
-- delta never names something that just became hidden.

CREATE TABLE library_changes (
    id        bigserial PRIMARY KEY,
    kind      text NOT NULL CHECK (kind IN ('track', 'album', 'artist', 'annotation', 'everything')),
    entity_id uuid,
    -- For annotations: whose. Others never see them.
    user_id   uuid,
    xid       xid8 NOT NULL DEFAULT pg_current_xact_id(),
    at        timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX library_changes_xid_idx ON library_changes (xid);

ALTER TABLE library_changes ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON library_changes FOR SELECT TO acetate_app
    USING (user_id IS NULL OR user_id = (SELECT app_viewer()));
REVOKE INSERT, UPDATE, DELETE ON library_changes FROM acetate_app;

-- How far the log was trimmed (`acetate` deletes old changes at startup): a client whose
-- copy is older than this gets the whole catalog.
CREATE TABLE library_changes_trimmed (
    one    boolean PRIMARY KEY DEFAULT true CHECK (one),
    before xid8 NOT NULL
);
REVOKE INSERT, UPDATE, DELETE ON library_changes_trimmed FROM acetate_app;

CREATE FUNCTION log_change(p_kind text, p_id uuid, p_user uuid DEFAULT NULL) RETURNS void
LANGUAGE sql SECURITY DEFINER SET search_path = public, pg_temp
AS $$ INSERT INTO library_changes (kind, entity_id, user_id) VALUES (p_kind, p_id, p_user) $$;

-- ---- Tracks ----

CREATE FUNCTION tracks_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND OLD.is_restricted IS DISTINCT FROM NEW.is_restricted THEN
        PERFORM log_change('everything', NULL);
    END IF;
    PERFORM log_change('track', COALESCE(NEW.id, OLD.id));
    RETURN NULL;
END
$$;
CREATE TRIGGER tracks_log_change AFTER INSERT OR UPDATE OR DELETE ON tracks
    FOR EACH ROW EXECUTE FUNCTION tracks_log_change();

-- A credit changes the track (its credits) and the artist (its track count).
CREATE FUNCTION track_credits_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF TG_OP <> 'INSERT' THEN
        PERFORM log_change('track', OLD.track_id);
        PERFORM log_change('artist', OLD.artist_id);
    END IF;
    IF TG_OP <> 'DELETE' THEN
        PERFORM log_change('track', NEW.track_id);
        PERFORM log_change('artist', NEW.artist_id);
    END IF;
    RETURN NULL;
END
$$;
CREATE TRIGGER track_credits_log_change AFTER INSERT OR UPDATE OR DELETE ON track_credits
    FOR EACH ROW EXECUTE FUNCTION track_credits_log_change();

-- A track on an album changes both (the track's albums, the album's counts).
CREATE FUNCTION album_tracks_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF TG_OP <> 'INSERT' THEN
        PERFORM log_change('track', OLD.track_id);
        PERFORM log_change('album', OLD.album_id);
    END IF;
    IF TG_OP <> 'DELETE' THEN
        PERFORM log_change('track', NEW.track_id);
        PERFORM log_change('album', NEW.album_id);
    END IF;
    RETURN NULL;
END
$$;
CREATE TRIGGER album_tracks_log_change AFTER INSERT OR UPDATE OR DELETE ON album_tracks
    FOR EACH ROW EXECUTE FUNCTION album_tracks_log_change();

-- A cover changes its track, or its album and the album's tracks (a track without a
-- cover of its own shows its album's).
CREATE FUNCTION covers_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF TG_OP <> 'INSERT' THEN
        PERFORM log_cover_owner(OLD.track_id, OLD.album_id);
    END IF;
    IF TG_OP <> 'DELETE' THEN
        PERFORM log_cover_owner(NEW.track_id, NEW.album_id);
    END IF;
    RETURN NULL;
END
$$;

CREATE FUNCTION log_cover_owner(p_track uuid, p_album uuid) RETURNS void
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF p_track IS NOT NULL THEN
        PERFORM log_change('track', p_track);
    END IF;
    IF p_album IS NOT NULL THEN
        PERFORM log_change('album', p_album);
        INSERT INTO library_changes (kind, entity_id)
        SELECT 'track', track_id FROM album_tracks WHERE album_id = p_album;
    END IF;
END
$$;
CREATE TRIGGER covers_log_change AFTER INSERT OR UPDATE OR DELETE ON covers
    FOR EACH ROW EXECUTE FUNCTION covers_log_change();

-- ---- Albums ----

CREATE FUNCTION albums_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    PERFORM log_change('album', COALESCE(NEW.id, OLD.id));
    RETURN NULL;
END
$$;
CREATE TRIGGER albums_log_change AFTER INSERT OR UPDATE OR DELETE ON albums
    FOR EACH ROW EXECUTE FUNCTION albums_log_change();

-- An album credit changes the album (its artists) and the artist (its album count).
CREATE FUNCTION album_credits_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF TG_OP <> 'INSERT' THEN
        PERFORM log_change('album', OLD.album_id);
        PERFORM log_change('artist', OLD.artist_id);
    END IF;
    IF TG_OP <> 'DELETE' THEN
        PERFORM log_change('album', NEW.album_id);
        PERFORM log_change('artist', NEW.artist_id);
    END IF;
    RETURN NULL;
END
$$;
CREATE TRIGGER album_credits_log_change AFTER INSERT OR UPDATE OR DELETE ON album_credits
    FOR EACH ROW EXECUTE FUNCTION album_credits_log_change();

-- ---- Artists ----

-- A renamed artist changes its albums too: an album's artist text is built from the
-- names (a track's is stored, and updated on the track, which logs it).
CREATE FUNCTION artists_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
DECLARE
    artist uuid := COALESCE(NEW.id, OLD.id);
BEGIN
    PERFORM log_change('artist', artist);
    IF TG_OP = 'UPDATE' AND OLD.name IS DISTINCT FROM NEW.name THEN
        INSERT INTO library_changes (kind, entity_id)
        SELECT DISTINCT 'album', album_id FROM album_credits WHERE artist_id = artist;
    END IF;
    RETURN NULL;
END
$$;
CREATE TRIGGER artists_log_change AFTER INSERT OR UPDATE OR DELETE ON artists
    FOR EACH ROW EXECUTE FUNCTION artists_log_change();

-- An alias changes its artist, and the tracks and albums credited under it (their
-- credits carry the alias's text).
CREATE FUNCTION artist_aliases_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
DECLARE
    alias uuid := COALESCE(NEW.id, OLD.id);
BEGIN
    PERFORM log_change('artist', COALESCE(NEW.artist_id, OLD.artist_id));
    INSERT INTO library_changes (kind, entity_id)
    SELECT DISTINCT 'track', track_id FROM track_credits WHERE alias_id = alias;
    INSERT INTO library_changes (kind, entity_id)
    SELECT DISTINCT 'album', album_id FROM album_credits WHERE alias_id = alias;
    RETURN NULL;
END
$$;
CREATE TRIGGER artist_aliases_log_change AFTER INSERT OR UPDATE OR DELETE ON artist_aliases
    FOR EACH ROW EXECUTE FUNCTION artist_aliases_log_change();

-- ---- A user's own plays, stars and ratings ----

CREATE FUNCTION annotations_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF TG_OP <> 'INSERT' THEN
        PERFORM log_change('annotation', COALESCE(OLD.track_id, OLD.album_id, OLD.artist_id), OLD.user_id);
    END IF;
    IF TG_OP <> 'DELETE' THEN
        PERFORM log_change('annotation', COALESCE(NEW.track_id, NEW.album_id, NEW.artist_id), NEW.user_id);
    END IF;
    RETURN NULL;
END
$$;
CREATE TRIGGER annotations_log_change AFTER INSERT OR UPDATE OR DELETE ON annotations
    FOR EACH ROW EXECUTE FUNCTION annotations_log_change();

-- ---- Who may see what: everything, again ----

CREATE FUNCTION log_everything() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    PERFORM log_change('everything', NULL);
    RETURN NULL;
END
$$;
CREATE TRIGGER track_access_log_change AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON track_access
    FOR EACH STATEMENT EXECUTE FUNCTION log_everything();
CREATE TRIGGER user_groups_log_change AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON user_groups
    FOR EACH STATEMENT EXECUTE FUNCTION log_everything();
CREATE TRIGGER group_permissions_log_change AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON group_permissions
    FOR EACH STATEMENT EXECUTE FUNCTION log_everything();
CREATE TRIGGER groups_log_change AFTER DELETE ON groups
    FOR EACH STATEMENT EXECUTE FUNCTION log_everything();
