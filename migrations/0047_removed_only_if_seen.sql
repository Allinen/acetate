-- A delta's `removed` named every changed thing the user can't see now (0046), including
-- things they never could: editing a hidden track's title told everyone else its id. Now
-- it names only what the user could see before: each change to who sees a track, an album
-- or an extra first records who could see it just before (`library_audience`), and a
-- delta says something is gone only if one of those records since the client's copy
-- includes the user (`seen_since`). Something the user never saw has no such record, or
-- none including them, so its id never reaches them.
--
-- Recorded by BEFORE triggers, once per thing per transaction (the first time it's
-- touched: the state before the transaction). A user's own groups are the same over a
-- whole delta (a change to them sends the whole catalog), so who they are is checked now.

CREATE TABLE library_audience (
    id        bigserial PRIMARY KEY,
    kind      text NOT NULL CHECK (kind IN ('track', 'album', 'extra')),
    entity_id uuid NOT NULL,
    -- Who could see it: every logged-in user; admins (false: nobody, e.g. a track turned
    -- into an extra); and members of these groups.
    everyone  boolean NOT NULL,
    admins    boolean NOT NULL,
    groups    uuid[] NOT NULL,
    xid       xid8 NOT NULL DEFAULT pg_current_xact_id(),
    at        timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX library_audience_entity_idx ON library_audience (entity_id, xid);
CREATE INDEX library_audience_xid_idx ON library_audience (xid);

-- No policy: the app reads it only through `seen_since`.
ALTER TABLE library_audience ENABLE ROW LEVEL SECURITY;
REVOKE ALL ON library_audience FROM acetate_app;

-- Records who sees a thing now (so, called before a change: who saw it before), unless
-- this transaction already did.
CREATE FUNCTION log_audience(p_kind text, p_id uuid) RETURNS void
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF p_id IS NULL OR EXISTS (SELECT 1 FROM library_audience
                               WHERE entity_id = p_id AND kind = p_kind AND xid = pg_current_xact_id()) THEN
        RETURN;
    END IF;
    IF p_kind = 'track' THEN
        -- As the tracks policy: a track turned into an extra is seen by nobody.
        INSERT INTO library_audience (kind, entity_id, everyone, admins, groups)
        SELECT 'track', t.id, t.became_extra_id IS NULL AND NOT t.is_restricted, t.became_extra_id IS NULL,
               CASE WHEN t.became_extra_id IS NULL AND t.is_restricted
                    THEN ARRAY(SELECT group_id FROM track_access WHERE track_id = t.id)
                    ELSE '{}' END
        FROM tracks t WHERE t.id = p_id;
    ELSIF p_kind = 'album' THEN
        -- As the albums policy: seen by whoever sees one of its tracks; with no tracks, by
        -- everyone; with only tracks turned into extras, by nobody. (A merged album is
        -- left to the merge: `db::library::merged`.)
        INSERT INTO library_audience (kind, entity_id, everyone, admins, groups)
        SELECT 'album', p_id,
               COALESCE(bool_or(t.became_extra_id IS NULL AND NOT t.is_restricted), true),
               COALESCE(bool_or(t.became_extra_id IS NULL), true),
               COALESCE(array_agg(DISTINCT ta.group_id)
                            FILTER (WHERE ta.group_id IS NOT NULL AND t.became_extra_id IS NULL AND t.is_restricted),
                        '{}')
        FROM album_tracks at
        JOIN tracks t ON t.id = at.track_id
        LEFT JOIN track_access ta ON ta.track_id = t.id
        WHERE at.album_id = p_id
        HAVING EXISTS (SELECT 1 FROM albums WHERE id = p_id AND merged_into IS NULL);
    ELSIF p_kind = 'extra' THEN
        INSERT INTO library_audience (kind, entity_id, everyone, admins, groups)
        SELECT 'extra', e.id, NOT e.is_restricted, true,
               CASE WHEN e.is_restricted THEN ARRAY(SELECT group_id FROM extra_access WHERE extra_id = e.id)
                    ELSE '{}' END
        FROM extras e WHERE e.id = p_id;
    END IF;
END
$$;

-- Of these, the ones the current user could see at some point since `p_since` (a catalog
-- version), before a change to who sees them.
CREATE FUNCTION seen_since(p_kind text, p_ids uuid[], p_since text) RETURNS SETOF uuid
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public, pg_temp
AS $$
    SELECT DISTINCT a.entity_id
    FROM library_audience a
    WHERE a.kind = p_kind AND a.entity_id = ANY (p_ids)
      AND a.xid >= pg_snapshot_xmin(p_since::pg_snapshot)
      AND NOT pg_visible_in_snapshot(a.xid, p_since::pg_snapshot)
      AND (a.everyone OR (a.admins AND app_sees_all()) OR a.groups && app_groups())
$$;
GRANT EXECUTE ON FUNCTION seen_since(text, uuid[], text) TO acetate_app;

-- ---- What changes who sees what ----

-- A track restricted or not, or turned into an extra (and back): it, and its albums.
CREATE FUNCTION tracks_log_audience() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    PERFORM log_audience('track', OLD.id);
    PERFORM log_audience('album', at.album_id) FROM album_tracks at WHERE at.track_id = OLD.id;
    RETURN NEW;
END
$$;
CREATE TRIGGER tracks_log_audience
BEFORE UPDATE OF is_restricted, became_extra_id ON tracks
FOR EACH ROW WHEN (OLD.is_restricted IS DISTINCT FROM NEW.is_restricted OR OLD.became_extra_id IS DISTINCT FROM NEW.became_extra_id)
EXECUTE FUNCTION tracks_log_audience();

-- A track's groups: it, and its albums.
CREATE FUNCTION track_access_log_audience() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
DECLARE
    v_track uuid;
BEGIN
    FOREACH v_track IN ARRAY ARRAY[OLD.track_id, NEW.track_id] LOOP
        IF v_track IS NOT NULL THEN
            PERFORM log_audience('track', v_track);
            PERFORM log_audience('album', at.album_id) FROM album_tracks at WHERE at.track_id = v_track;
        END IF;
    END LOOP;
    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER track_access_log_audience BEFORE INSERT OR UPDATE OR DELETE ON track_access
    FOR EACH ROW EXECUTE FUNCTION track_access_log_audience();

-- An album's tracks: the album (a track taken off it can hide it from some).
CREATE FUNCTION album_tracks_log_audience() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF TG_OP <> 'INSERT' THEN
        PERFORM log_audience('album', OLD.album_id);
    END IF;
    IF TG_OP <> 'DELETE' THEN
        PERFORM log_audience('album', NEW.album_id);
        RETURN NEW;
    END IF;
    RETURN OLD;
END
$$;
CREATE TRIGGER album_tracks_log_audience BEFORE INSERT OR UPDATE OR DELETE ON album_tracks
    FOR EACH ROW EXECUTE FUNCTION album_tracks_log_audience();

-- An extra restricted or not, and its groups.
CREATE FUNCTION extras_log_audience() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    PERFORM log_audience('extra', OLD.id);
    RETURN NEW;
END
$$;
CREATE TRIGGER extras_log_audience
BEFORE UPDATE OF is_restricted ON extras
FOR EACH ROW WHEN (OLD.is_restricted IS DISTINCT FROM NEW.is_restricted)
EXECUTE FUNCTION extras_log_audience();

CREATE FUNCTION extra_access_log_audience() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF TG_OP <> 'INSERT' THEN
        PERFORM log_audience('extra', OLD.extra_id);
    END IF;
    IF TG_OP <> 'DELETE' THEN
        PERFORM log_audience('extra', NEW.extra_id);
        RETURN NEW;
    END IF;
    RETURN OLD;
END
$$;
CREATE TRIGGER extra_access_log_audience BEFORE INSERT OR UPDATE OR DELETE ON extra_access
    FOR EACH ROW EXECUTE FUNCTION extra_access_log_audience();
