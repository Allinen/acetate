-- Extras get a description and a source (free text: a link, or where it came from in
-- words), and a track can become an extra (vocals imported as a track, say): the extra
-- gets its file (the same bytes), and the track stays, with everything on it (its file,
-- sources, plays), but nobody sees it anymore (`became_extra_id`, in its visibility rule,
-- so every list, the catalog and search leave it out). Making it a track again undoes it.
-- An album whose every track became an extra is hidden too (for everyone: it would be an
-- empty shell), and back with them. See `db::extras`.

ALTER TABLE extras
    ADD COLUMN description   text NOT NULL DEFAULT '',
    ADD COLUMN source        text NOT NULL DEFAULT '',
    -- The track it was made from (hidden while it's an extra).
    ADD COLUMN from_track_id uuid REFERENCES tracks (id);

ALTER TABLE tracks ADD COLUMN became_extra_id uuid REFERENCES extras (id);
CREATE INDEX tracks_became_extra_idx ON tracks (became_extra_id) WHERE became_extra_id IS NOT NULL;

-- ---- Tracks: one that became an extra is seen by nobody ----
DROP POLICY visible ON tracks;
CREATE POLICY visible ON tracks TO acetate_app USING (
    became_extra_id IS NULL
    AND (NOT is_restricted OR (SELECT app_sees_all()) OR restricted_track_visible(id))
    AND ((SELECT current_app_share()) IS NULL OR id IN (SELECT unnest(app_share_tracks())))
);

-- ---- Albums: a track that became an extra counts as hidden ----
ALTER TABLE albums ADD COLUMN all_tracks_extras boolean NOT NULL DEFAULT false;

CREATE OR REPLACE FUNCTION refresh_album_restricted(p_album_id uuid) RETURNS void
LANGUAGE sql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
    UPDATE albums a
    SET all_tracks_restricted = x.restricted, all_tracks_extras = x.extras
    FROM (SELECT EXISTS (SELECT 1 FROM album_tracks WHERE album_id = p_album_id)
                 AND NOT EXISTS (SELECT 1 FROM album_tracks at JOIN tracks t ON t.id = at.track_id
                                 WHERE at.album_id = p_album_id AND NOT t.is_restricted AND t.became_extra_id IS NULL) AS restricted,
                 EXISTS (SELECT 1 FROM album_tracks WHERE album_id = p_album_id)
                 AND NOT EXISTS (SELECT 1 FROM album_tracks at JOIN tracks t ON t.id = at.track_id
                                 WHERE at.album_id = p_album_id AND t.became_extra_id IS NULL) AS extras) x
    WHERE a.id = p_album_id AND (a.all_tracks_restricted <> x.restricted OR a.all_tracks_extras <> x.extras)
$$;

DROP TRIGGER tracks_refresh_album_restricted ON tracks;
CREATE TRIGGER tracks_refresh_album_restricted
AFTER UPDATE OF is_restricted, became_extra_id ON tracks
FOR EACH ROW WHEN (OLD.is_restricted IS DISTINCT FROM NEW.is_restricted OR OLD.became_extra_id IS DISTINCT FROM NEW.became_extra_id)
EXECUTE FUNCTION tracks_refresh_album_restricted();

DROP POLICY visible ON albums;
CREATE POLICY visible ON albums TO acetate_app USING (
    NOT all_tracks_extras
    AND CASE WHEN (SELECT current_app_share()) IS NULL
             THEN (merged_into IS NULL AND NOT all_tracks_restricted)
                  OR (SELECT app_sees_all())
                  OR album_visible_by_tracks(id, merged_into)
             ELSE id = (SELECT app_share_album())
        END
);

-- ---- The catalog: a track leaving everyone's sight (or coming back) is everything ----
CREATE OR REPLACE FUNCTION tracks_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND (OLD.is_restricted IS DISTINCT FROM NEW.is_restricted
                             OR OLD.became_extra_id IS DISTINCT FROM NEW.became_extra_id) THEN
        PERFORM log_change('everything', NULL);
    END IF;
    PERFORM log_change('track', COALESCE(NEW.id, OLD.id));
    RETURN NULL;
END
$$;

-- ---- Becoming an extra, and back ----
-- As the system (the track leaves the user's sight halfway, which its visibility rule
-- would refuse), after checking what the rules would: the user sees the track (or the
-- extra).
CREATE FUNCTION track_became_extra(p_track uuid, p_extra uuid) RETURNS void
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM tracks t
        WHERE t.id = p_track AND t.became_extra_id IS NULL AND t.merged_into IS NULL
          AND (NOT t.is_restricted OR app_sees_all() OR restricted_track_visible(t.id))
          AND current_app_share() IS NULL
    ) THEN
        RAISE EXCEPTION 'no such track' USING ERRCODE = 'P0002';
    END IF;
    UPDATE extras SET from_track_id = p_track WHERE id = p_extra;
    UPDATE tracks SET became_extra_id = p_extra WHERE id = p_track;
END
$$;

-- Returns the track, a track again; the extra is taken off (kept).
CREATE FUNCTION extra_back_to_track(p_extra uuid) RETURNS uuid
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
DECLARE
    v_track uuid;
BEGIN
    SELECT e.from_track_id INTO v_track FROM extras e
    WHERE e.id = p_extra AND e.from_track_id IS NOT NULL
      AND (NOT e.is_restricted OR app_sees_all() OR restricted_extra_visible(e.id))
      AND current_app_share() IS NULL;
    IF v_track IS NULL THEN
        RAISE EXCEPTION 'no such extra made from a track' USING ERRCODE = 'P0002';
    END IF;
    UPDATE tracks SET became_extra_id = NULL WHERE id = v_track AND became_extra_id = p_extra;
    UPDATE extras SET from_track_id = NULL, hidden_at = now() WHERE id = p_extra;
    RETURN v_track;
END
$$;
