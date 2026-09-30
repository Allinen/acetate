-- One more flag the server raises itself: a track whose sources say too little to find
-- where it came from again (for "fix later": imported now, a better file or source
-- found later). Like the other automatic flags (0030): raised when a track comes to
-- have only such sources, settled ("fixed") when it gets a real one, and marking it done
-- ("that's all there is") stays.
--
-- Says too little: no sources at all (trashed ones don't count), or only custom ones
-- whose newest snapshot has nothing but a label (`{"label": "vk"}`: no description, no
-- link).

UPDATE review_kinds SET position = position + 1 WHERE position >= 10;
INSERT INTO review_kinds (kind, name, automatic, checked, position)
VALUES ('weak_source', 'Source says too little', true, true, 10);

DROP INDEX review_flags_one_open_automatic;
CREATE UNIQUE INDEX review_flags_one_open_automatic ON review_flags (track_id, kind)
    WHERE resolved_at IS NULL AND kind IN (
        'missing_title', 'missing_artists', 'missing_album', 'missing_cover', 'missing_date', 'credits_in_title',
        'weak_source'
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
        IF NOT EXISTS (
            SELECT 1 FROM sources s
            WHERE s.track_id = p_track AND s.trashed_at IS NULL
              AND NOT (s.platform = 'custom' AND (
                  SELECT COALESCE(btrim(ss.data ->> 'description'), '') = '' AND COALESCE(btrim(ss.data ->> 'url'), '') = ''
                  FROM source_snapshots ss WHERE ss.source_id = s.id
                  ORDER BY ss.captured_at DESC, ss.id DESC LIMIT 1
              ))
        ) THEN
            lacks := lacks || 'weak_source'::text;
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
        'missing_title', 'missing_artists', 'missing_album', 'missing_cover', 'missing_date', 'credits_in_title',
        'weak_source'
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

-- A track's sources and their snapshots change what it's missing too.
CREATE FUNCTION check_source_track() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF TG_TABLE_NAME = 'sources' THEN
        IF TG_OP <> 'INSERT' AND OLD.track_id IS NOT NULL THEN
            PERFORM check_track(OLD.track_id);
        END IF;
        IF TG_OP <> 'DELETE' AND NEW.track_id IS NOT NULL THEN
            PERFORM check_track(NEW.track_id);
        END IF;
    ELSE
        PERFORM check_track(s.track_id) FROM sources s
        WHERE s.id = CASE WHEN TG_OP = 'DELETE' THEN OLD.source_id ELSE NEW.source_id END
          AND s.track_id IS NOT NULL;
    END IF;
    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER sources_check_missing AFTER INSERT OR UPDATE OR DELETE ON sources
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION check_source_track();
CREATE CONSTRAINT TRIGGER source_snapshots_check_missing AFTER INSERT OR UPDATE OR DELETE ON source_snapshots
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION check_source_track();

-- Every track, for the new check.
SELECT count(check_track(id)) FROM tracks;
