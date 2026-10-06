-- What changed about stored blobs, for mirrors (`GET /api/mirror?since=`): a copy of the
-- blob store on another machine asks what changed since its last look instead of listing
-- every blob again.
--
-- Like `library_changes` (migration 0029): one row per change, the hash and the
-- transaction; a mirror's version is a snapshot, so a transaction that got its id earlier
-- but committed later isn't missed. What's logged is only *which* blob; its state (in use
-- as audio, a cover, an extra, not imported, a previous file, trashed, or gone) is worked
-- out when asked, from the tables that refer to it. So every change that can move a blob
-- from one state to another logs it: the blob stored, moved to another volume or deleted,
-- a file trashed or purged, a track playing another file, a cover or an extra's file
-- added, trashed or removed, a file that didn't import resolved.
--
-- Each change also notifies `blob_changes`, so a mirror waiting for one (`&wait=`) hears
-- of it as soon as it's committed.
--
-- Written by triggers only, as the owner (SECURITY DEFINER); only the system reads it,
-- like `blobs` (migration 0018): it names hidden tracks' files too.

CREATE TABLE blob_changes (
    id     bigserial PRIMARY KEY,
    sha256 text NOT NULL,
    xid    xid8 NOT NULL DEFAULT pg_current_xact_id(),
    at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX blob_changes_xid_idx ON blob_changes (xid);

ALTER TABLE blob_changes ENABLE ROW LEVEL SECURITY;
REVOKE ALL ON blob_changes FROM acetate_app;

-- How far the log was trimmed (at startup, with the catalog's): a mirror whose version is
-- older gets the whole list.
CREATE TABLE blob_changes_trimmed (
    one    boolean PRIMARY KEY DEFAULT true CHECK (one),
    before xid8 NOT NULL
);
ALTER TABLE blob_changes_trimmed ENABLE ROW LEVEL SECURITY;
REVOKE ALL ON blob_changes_trimmed FROM acetate_app;

CREATE FUNCTION log_blob_change(p_sha256 text) RETURNS void
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF p_sha256 IS NOT NULL THEN
        INSERT INTO blob_changes (sha256) VALUES (p_sha256);
        -- Sent on commit; the same notification twice in a transaction is sent once.
        PERFORM pg_notify('blob_changes', '');
    END IF;
END
$$;

-- A file's blob, by the file's id.
CREATE FUNCTION log_file_blob_change(p_file_id uuid) RETURNS void
LANGUAGE sql SECURITY DEFINER SET search_path = public, pg_temp
AS $$ SELECT log_blob_change(sha256) FROM files WHERE id = p_file_id $$;

-- ---- Blobs: stored, moved to another volume, deleted ----

CREATE FUNCTION blobs_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    PERFORM log_blob_change(COALESCE(NEW.sha256, OLD.sha256));
    RETURN NULL;
END
$$;
CREATE TRIGGER blobs_log_change AFTER INSERT OR UPDATE OF volume OR DELETE ON blobs
    FOR EACH ROW EXECUTE FUNCTION blobs_log_change();

-- ---- Files: added, trashed, purged ----

CREATE FUNCTION files_log_blob_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    PERFORM log_blob_change(COALESCE(NEW.sha256, OLD.sha256));
    RETURN NULL;
END
$$;
CREATE TRIGGER files_log_blob_change AFTER INSERT OR UPDATE OF trashed_at, purged_at OR DELETE ON files
    FOR EACH ROW EXECUTE FUNCTION files_log_blob_change();

-- ---- Tracks: which file one plays (the one it gave up may be a previous file now) ----

CREATE FUNCTION tracks_log_blob_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF TG_OP <> 'INSERT' THEN
        PERFORM log_file_blob_change(OLD.file_id);
    END IF;
    IF TG_OP <> 'DELETE' THEN
        PERFORM log_file_blob_change(NEW.file_id);
    END IF;
    RETURN NULL;
END
$$;
CREATE TRIGGER tracks_log_blob_change AFTER INSERT OR UPDATE OF file_id OR DELETE ON tracks
    FOR EACH ROW EXECUTE FUNCTION tracks_log_blob_change();

CREATE FUNCTION track_file_history_log_blob_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    PERFORM log_file_blob_change(COALESCE(NEW.old_file_id, OLD.old_file_id));
    RETURN NULL;
END
$$;
CREATE TRIGGER track_file_history_log_blob_change AFTER INSERT OR DELETE ON track_file_history
    FOR EACH ROW EXECUTE FUNCTION track_file_history_log_blob_change();

-- ---- Covers: added, trashed, purged ----

CREATE FUNCTION covers_log_blob_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF TG_OP <> 'INSERT' THEN
        PERFORM log_blob_change(OLD.blob_sha256);
    END IF;
    IF TG_OP <> 'DELETE' AND NEW.blob_sha256 IS DISTINCT FROM OLD.blob_sha256 THEN
        PERFORM log_blob_change(NEW.blob_sha256);
    END IF;
    RETURN NULL;
END
$$;
CREATE TRIGGER covers_log_blob_change AFTER INSERT OR UPDATE OF blob_sha256, trashed_at OR DELETE ON covers
    FOR EACH ROW EXECUTE FUNCTION covers_log_blob_change();

-- ---- Extras' files and their thumbnails ----

CREATE FUNCTION extra_files_log_blob_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    IF TG_OP <> 'INSERT' THEN
        PERFORM log_blob_change(OLD.sha256);
        PERFORM log_blob_change(OLD.thumb_sha256);
    END IF;
    IF TG_OP <> 'DELETE' THEN
        PERFORM log_blob_change(NEW.sha256);
        PERFORM log_blob_change(NEW.thumb_sha256);
    END IF;
    RETURN NULL;
END
$$;
CREATE TRIGGER extra_files_log_blob_change AFTER INSERT OR UPDATE OF sha256, thumb_sha256 OR DELETE ON extra_files
    FOR EACH ROW EXECUTE FUNCTION extra_files_log_blob_change();

-- ---- Files that didn't import (and the covers sent with them) ----

CREATE FUNCTION ingest_problems_log_blob_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
DECLARE
    sha text;
BEGIN
    IF TG_OP <> 'INSERT' THEN
        FOREACH sha IN ARRAY OLD.cover_blobs || OLD.sha256 LOOP
            PERFORM log_blob_change(sha);
        END LOOP;
    END IF;
    IF TG_OP <> 'DELETE' THEN
        FOREACH sha IN ARRAY NEW.cover_blobs || NEW.sha256 LOOP
            PERFORM log_blob_change(sha);
        END LOOP;
    END IF;
    RETURN NULL;
END
$$;
CREATE TRIGGER ingest_problems_log_blob_change
    AFTER INSERT OR UPDATE OF sha256, cover_blobs, resolved_at, track_id, audio_purged_at OR DELETE ON ingest_problems
    FOR EACH ROW EXECUTE FUNCTION ingest_problems_log_blob_change();
