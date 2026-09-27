-- Faster row-level security for artists and albums.
--
-- Their policies looked through the credit / album_tracks tables (and those through
-- tracks) for every row. At 185k tracks that made "all artists" take 1.5 s, artist and
-- album search 7-9x slower, and the whole catalog (`GET /api/catalog`) 24 s. Tracks were
-- never the problem: their policy settles most rows on a column (`NOT is_restricted`).
-- Now artists and albums do the same.
--
-- The expensive checks that remain are in plpgsql functions. Postgres plans subqueries in a
-- policy up front, even for rows an earlier OR branch already let through; a function
-- call only runs when it's reached. They're SECURITY INVOKER (the default), so the
-- queries inside still see only what the user may see.

-- ---- Artists: visible to everyone logged in ----
-- Before, an artist credited only on hidden tracks was hidden too. Its name alone says
-- little (it might be an artist whose credits moved elsewhere), so it's visible now; its
-- tracks and albums stay hidden as before. A share link still shows only the artists of
-- what it shares.

CREATE FUNCTION artist_credited_in_view(p_artist_id uuid, p_merged_into uuid) RETURNS boolean
LANGUAGE plpgsql STABLE AS $$
BEGIN
    RETURN EXISTS (SELECT 1 FROM track_credits c WHERE c.artist_id = p_artist_id)
        OR EXISTS (SELECT 1 FROM album_credits c WHERE c.artist_id = p_artist_id)
        OR (p_merged_into IS NOT NULL
            AND (EXISTS (SELECT 1 FROM track_credits c WHERE c.artist_id = p_merged_into)
                 OR EXISTS (SELECT 1 FROM album_credits c WHERE c.artist_id = p_merged_into)));
END
$$;
GRANT EXECUTE ON FUNCTION artist_credited_in_view(uuid, uuid) TO acetate_app;

DROP POLICY visible ON artists;
CREATE POLICY visible ON artists TO acetate_app USING (
    (SELECT current_app_share()) IS NULL
    OR artist_credited_in_view(id, merged_into)
);

-- ---- Albums: a flag like tracks.is_restricted ----
-- `all_tracks_restricted`: the album has tracks, and every one of them is restricted.
-- Any other album is visible to everyone logged in, as before: it has an unrestricted
-- track (visible to all), or none at all (it reveals nothing). Only flagged albums, and
-- merged ones (visible when the album they went into is), need the real check: a track
-- of theirs the user can see. Kept up to date by the triggers below, as the system (the
-- user may not see the tracks that decide it).

ALTER TABLE albums ADD COLUMN all_tracks_restricted boolean NOT NULL DEFAULT false;

CREATE FUNCTION refresh_album_restricted(p_album_id uuid) RETURNS void
LANGUAGE sql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
    UPDATE albums a
    SET all_tracks_restricted = x.value
    FROM (SELECT EXISTS (SELECT 1 FROM album_tracks WHERE album_id = p_album_id)
                 AND NOT EXISTS (SELECT 1 FROM album_tracks at JOIN tracks t ON t.id = at.track_id
                                 WHERE at.album_id = p_album_id AND NOT t.is_restricted) AS value) x
    WHERE a.id = p_album_id AND a.all_tracks_restricted <> x.value
$$;

CREATE FUNCTION album_tracks_refresh_restricted() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF TG_OP IN ('UPDATE', 'DELETE') THEN
        PERFORM refresh_album_restricted(OLD.album_id);
    END IF;
    IF TG_OP IN ('INSERT', 'UPDATE') THEN
        PERFORM refresh_album_restricted(NEW.album_id);
    END IF;
    RETURN NULL;
END
$$;

CREATE TRIGGER album_tracks_refresh_restricted
AFTER INSERT OR DELETE OR UPDATE OF album_id, track_id ON album_tracks
FOR EACH ROW EXECUTE FUNCTION album_tracks_refresh_restricted();

CREATE FUNCTION tracks_refresh_album_restricted() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    PERFORM refresh_album_restricted(at.album_id) FROM album_tracks at WHERE at.track_id = NEW.id;
    RETURN NULL;
END
$$;

CREATE TRIGGER tracks_refresh_album_restricted
AFTER UPDATE OF is_restricted ON tracks
FOR EACH ROW WHEN (OLD.is_restricted IS DISTINCT FROM NEW.is_restricted)
EXECUTE FUNCTION tracks_refresh_album_restricted();

UPDATE albums a SET all_tracks_restricted = true
WHERE EXISTS (SELECT 1 FROM album_tracks WHERE album_id = a.id)
  AND NOT EXISTS (SELECT 1 FROM album_tracks at JOIN tracks t ON t.id = at.track_id
                  WHERE at.album_id = a.id AND NOT t.is_restricted);

-- The real check, for flagged and merged albums (the old policy's rule).
CREATE FUNCTION album_visible_by_tracks(p_album_id uuid, p_merged_into uuid) RETURNS boolean
LANGUAGE plpgsql STABLE AS $$
BEGIN
    IF p_merged_into IS NULL THEN
        RETURN EXISTS (SELECT 1 FROM album_tracks WHERE album_id = p_album_id)
            OR NOT album_has_tracks(p_album_id);
    END IF;
    RETURN EXISTS (SELECT 1 FROM album_tracks WHERE album_id = p_merged_into)
        OR NOT album_has_tracks(p_merged_into);
END
$$;
GRANT EXECUTE ON FUNCTION album_visible_by_tracks(uuid, uuid) TO acetate_app;

DROP POLICY visible ON albums;
CREATE POLICY visible ON albums TO acetate_app USING (
    CASE WHEN (SELECT current_app_share()) IS NULL
         THEN (merged_into IS NULL AND NOT all_tracks_restricted)
              OR (SELECT app_sees_all())
              OR album_visible_by_tracks(id, merged_into)
         ELSE id = (SELECT app_share_album())
    END
);
