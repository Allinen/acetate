-- Review flags for what a track is missing (a title, artists, an album, a cover, a
-- release date), raised and settled by the server itself: clients don't each work it
-- out, and every client sees the same list.
--
-- The kinds of flag are a table now (like the other fixed lists): their names for
-- people, whether the server raises them itself ("automatic"), and for those, whether
-- it checks for them at all (the release date isn't checked by default: many real
-- releases don't have a known one). `acetate review-checks` turns checks on and off.
--
-- How an automatic flag behaves:
--   * it's raised when the track starts missing something (at import, or when an edit
--     takes it away: the last credit removed, the cover trashed);
--   * the server settles it ("fixed") when the thing is there again;
--   * a person can settle it too ("fine as it is": a track that really has no cover).
--     That stays settled while the track goes on missing it: a new flag is raised only
--     when it's fixed and later goes missing again.
-- For the last rule, the server keeps what each track was missing when last checked
-- (`track_missing`): a flag is raised only for what's newly missing.
--
-- Checks run when the transaction commits (deferred triggers): an import that makes a
-- track and then its credits, album and cover is checked once, complete, instead of
-- raising flags that are settled a moment later.

CREATE TABLE review_kinds (
    kind      text PRIMARY KEY,
    name      text NOT NULL,
    automatic boolean NOT NULL DEFAULT false,
    -- For automatic kinds: whether the server checks for it.
    checked   boolean NOT NULL DEFAULT true,
    position  smallint NOT NULL
);
INSERT INTO review_kinds (kind, name, automatic, checked, position) VALUES
    ('fix_later',          'Fix later',          false, true,  1),
    ('possible_duplicate', 'Possible duplicate', false, true,  2),
    ('ambiguous_artist',   'Ambiguous artist',   false, true,  3),
    ('merged_artist',      'Artist merged',      false, true,  4),
    ('missing_title',      'No title',           true,  true,  5),
    ('missing_artists',    'No artists',         true,  true,  6),
    ('missing_album',      'No album',           true,  true,  7),
    ('missing_cover',      'No cover',           true,  true,  8),
    ('missing_date',       'No release date',    true,  false, 9);
-- Read by the app; changed only by the owner (the CLI).
REVOKE INSERT, UPDATE, DELETE ON review_kinds FROM acetate_app;

ALTER TABLE review_flags DROP CONSTRAINT review_flags_kind_check;
ALTER TABLE review_flags ADD CONSTRAINT review_flags_kind_fkey FOREIGN KEY (kind) REFERENCES review_kinds (kind);

-- One open automatic flag per track and kind (two transactions checking the same track
-- at once don't raise it twice).
CREATE UNIQUE INDEX review_flags_one_open_missing ON review_flags (track_id, kind)
    WHERE resolved_at IS NULL AND kind LIKE 'missing\_%';

-- What each track was missing when last checked. Only the server's functions use it.
CREATE TABLE track_missing (
    track_id uuid PRIMARY KEY REFERENCES tracks (id) ON DELETE CASCADE,
    missing  text[] NOT NULL DEFAULT '{}'
);
ALTER TABLE track_missing ENABLE ROW LEVEL SECURITY;
REVOKE ALL ON track_missing FROM acetate_app;

-- Checks one track: what it's missing now (as its page and the catalog show it: its own
-- cover, else its main album's; a merged track misses nothing), flags what's newly
-- missing, settles the flags of what's there again.
CREATE FUNCTION check_track(p_track uuid) RETURNS void
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

    -- Newly missing: flagged.
    INSERT INTO review_flags (kind, track_id)
    SELECT k, p_track FROM unnest(missing) k
    WHERE k <> ALL (before)
    ON CONFLICT (track_id, kind) WHERE resolved_at IS NULL AND kind LIKE 'missing\_%' DO NOTHING;
    -- There again (or not checked anymore): its open flag is settled.
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

-- A cover changes its track, or its album's tracks.
CREATE FUNCTION check_cover_owner(p_track uuid, p_album uuid) RETURNS void
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF p_track IS NOT NULL THEN
        PERFORM check_track(p_track);
    END IF;
    IF p_album IS NOT NULL THEN
        PERFORM check_track(at.track_id) FROM album_tracks at WHERE at.album_id = p_album;
    END IF;
END
$$;

CREATE FUNCTION check_tracks_after_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    CASE TG_TABLE_NAME
        WHEN 'tracks' THEN
            PERFORM check_track(NEW.id);
        WHEN 'track_credits', 'album_tracks' THEN
            IF TG_OP <> 'INSERT' THEN
                PERFORM check_track(OLD.track_id);
            END IF;
            IF TG_OP <> 'DELETE' THEN
                PERFORM check_track(NEW.track_id);
            END IF;
        WHEN 'covers' THEN
            IF TG_OP <> 'INSERT' THEN
                PERFORM check_cover_owner(OLD.track_id, OLD.album_id);
            END IF;
            IF TG_OP <> 'DELETE' THEN
                PERFORM check_cover_owner(NEW.track_id, NEW.album_id);
            END IF;
        WHEN 'albums' THEN
            -- Merged away, or became a single: its tracks' main album may change.
            PERFORM check_track(at.track_id) FROM album_tracks at WHERE at.album_id = NEW.id;
    END CASE;
    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER tracks_check_missing AFTER INSERT OR UPDATE OF title, release_date, merged_into ON tracks
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION check_tracks_after_change();
CREATE CONSTRAINT TRIGGER track_credits_check_missing AFTER INSERT OR UPDATE OR DELETE ON track_credits
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION check_tracks_after_change();
CREATE CONSTRAINT TRIGGER album_tracks_check_missing AFTER INSERT OR UPDATE OR DELETE ON album_tracks
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION check_tracks_after_change();
CREATE CONSTRAINT TRIGGER covers_check_missing AFTER INSERT OR UPDATE OR DELETE ON covers
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION check_tracks_after_change();
CREATE CONSTRAINT TRIGGER albums_check_missing AFTER UPDATE OF kind, merged_into ON albums
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION check_tracks_after_change();

-- Every track, once, now.
SELECT count(check_track(id)) FROM tracks;

-- A track's flags are part of the catalog (`issues`): a flag raised or settled is a
-- change to its track, for deltas. (Created after the pass above, which instead asks
-- every client for the whole catalog again, just below.)
CREATE FUNCTION review_flags_log_change() RETURNS trigger
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
CREATE TRIGGER review_flags_log_change AFTER INSERT OR UPDATE OR DELETE ON review_flags
    FOR EACH ROW EXECUTE FUNCTION review_flags_log_change();

SELECT log_change('everything', NULL);
