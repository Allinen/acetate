-- One more flag the server raises itself: credits in the title ("Song (feat. Ann)",
-- "Song w/ Ann", "Song [prod. Bo]"). The artists belong in the credits; a title like
-- that says tagging isn't finished. Like the "missing" flags (see 0030): raised when the
-- title gets one, settled ("fixed") when it's gone, and marking it done ("it really is
-- the title") stays.
--
-- What counts: "feat", "ft", "featuring", "w/" (not "w/o"), "prod", "produced by", as a
-- word (after the start, a space or an opening bracket or dash, and before a space or the
-- end), any case, with or without the dot.

UPDATE review_kinds SET position = position + 1 WHERE position >= 6;
INSERT INTO review_kinds (kind, name, automatic, checked, position)
VALUES ('credits_in_title', 'Credits in the title', true, true, 6);

-- One open flag per track and kind, for every automatic kind (the index in 0030 covered
-- the "missing" ones by their name). A new automatic kind is added to this list.
DROP INDEX review_flags_one_open_missing;
CREATE UNIQUE INDEX review_flags_one_open_automatic ON review_flags (track_id, kind)
    WHERE resolved_at IS NULL AND kind IN (
        'missing_title', 'missing_artists', 'missing_album', 'missing_cover', 'missing_date', 'credits_in_title'
    );

CREATE OR REPLACE FUNCTION check_track(p_track uuid) RETURNS void
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
DECLARE
    t record;
    main_album uuid;
    lacks text[] := '{}';
    missing text[];
    before text[];
BEGIN
    SELECT id, title, release_date, merged_into INTO t FROM tracks WHERE id = p_track;
    IF NOT FOUND THEN
        RETURN;
    END IF;
    IF t.merged_into IS NULL THEN
        -- The main album, as the catalog chooses it: a real album before a single, then
        -- the oldest.
        SELECT at.album_id INTO main_album
        FROM album_tracks at
        JOIN albums a ON a.id = at.album_id AND a.merged_into IS NULL
        WHERE at.track_id = p_track
        ORDER BY a.kind = 'single', a.id
        LIMIT 1;
        IF btrim(t.title) = '' THEN
            lacks := lacks || 'missing_title'::text;
        END IF;
        IF t.title ~* '(^|[[:space:](\[{-])(feat\.?|ft\.?|featuring|w/(?!o)|prod\.?|produced by)([[:space:]]|$)' THEN
            lacks := lacks || 'credits_in_title'::text;
        END IF;
        IF NOT EXISTS (SELECT 1 FROM track_credits WHERE track_id = p_track) THEN
            lacks := lacks || 'missing_artists'::text;
        END IF;
        IF main_album IS NULL THEN
            lacks := lacks || 'missing_album'::text;
        END IF;
        IF NOT EXISTS (
            SELECT 1 FROM covers
            WHERE trashed_at IS NULL AND (track_id = p_track OR (main_album IS NOT NULL AND album_id = main_album))
        ) THEN
            lacks := lacks || 'missing_cover'::text;
        END IF;
        IF COALESCE(btrim(t.release_date), '') = '' THEN
            lacks := lacks || 'missing_date'::text;
        END IF;
    END IF;
    -- Only what's checked for.
    missing := ARRAY(SELECT k.kind FROM review_kinds k WHERE k.automatic AND k.checked AND k.kind = ANY (lacks));
    before := COALESCE((SELECT m.missing FROM track_missing m WHERE m.track_id = p_track), '{}');

    -- Newly found: flagged.
    INSERT INTO review_flags (kind, track_id)
    SELECT k, p_track FROM unnest(missing) k
    WHERE k <> ALL (before)
    ON CONFLICT (track_id, kind) WHERE resolved_at IS NULL AND kind IN (
        'missing_title', 'missing_artists', 'missing_album', 'missing_cover', 'missing_date', 'credits_in_title'
    ) DO NOTHING;
    -- Gone (or not checked anymore): its open flag is settled.
    UPDATE review_flags f
    SET resolved_at = now(), resolved_by = current_app_user(), resolution = 'fixed'
    FROM review_kinds k
    WHERE f.track_id = p_track AND f.resolved_at IS NULL
      AND k.kind = f.kind AND k.automatic AND f.kind <> ALL (missing);

    INSERT INTO track_missing (track_id, missing) VALUES (p_track, missing)
    ON CONFLICT (track_id) DO UPDATE SET missing = EXCLUDED.missing
    WHERE track_missing.missing IS DISTINCT FROM EXCLUDED.missing;
END
$$;

-- Every track, for the new check.
SELECT count(check_track(id)) FROM tracks;

-- Where a track went: itself, or (merged) the track it was merged into, following merges
-- of merges. For flags naming tracks that were merged since (see `db::review::list`).
-- SECURITY DEFINER: merged tracks are hidden from the app; this only gives an id, and
-- the track it names is then read as the user may see it.
CREATE FUNCTION final_track(p_track uuid) RETURNS uuid
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public, pg_temp
AS $$
    WITH RECURSIVE chain (id, next, depth) AS (
        SELECT id, merged_into, 0 FROM tracks WHERE id = p_track
        UNION ALL
        SELECT t.id, t.merged_into, c.depth + 1 FROM chain c JOIN tracks t ON t.id = c.next WHERE c.depth < 50
    )
    SELECT id FROM chain WHERE next IS NULL LIMIT 1
$$;
GRANT EXECUTE ON FUNCTION final_track(uuid) TO acetate_app;
