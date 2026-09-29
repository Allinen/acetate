-- Groupings: tracks and albums gathered under a name, apart from albums and credits. The
-- first kind is `collective`: an account on a platform that several artists post their
-- songs under (friends sharing a channel). Credits say who made a song; a collective
-- didn't, so it isn't an artist: its tracks are in its grouping instead. Other kinds
-- (`era`, `project`) come the same way. A track or album can be in several groupings.
--
-- A grouping has sources like an artist (the platform accounts it is): ingest puts a song
-- whose artist id is one of them into the grouping, instead of crediting an artist.
--
-- Nothing is deleted: a grouping stays (empty, if everything's taken out of it), and every
-- change (made, renamed, added to, taken from) is in `grouping_log`.

CREATE TABLE groupings (
    id          uuid PRIMARY KEY DEFAULT uuidv7(),
    kind        text NOT NULL CHECK (kind ~ '^[a-z][a-z_]*$'),
    name        text NOT NULL CHECK (btrim(name) <> ''),
    description text NOT NULL DEFAULT '',
    created_at  timestamptz NOT NULL DEFAULT now(),
    created_by  uuid REFERENCES users (id) ON DELETE SET NULL DEFAULT current_app_user()
);

CREATE TABLE grouping_tracks (
    grouping_id uuid NOT NULL REFERENCES groupings (id),
    track_id    uuid NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
    added_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (grouping_id, track_id)
);
CREATE INDEX grouping_tracks_track_idx ON grouping_tracks (track_id);

CREATE TABLE grouping_albums (
    grouping_id uuid NOT NULL REFERENCES groupings (id),
    album_id    uuid NOT NULL REFERENCES albums (id) ON DELETE CASCADE,
    added_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (grouping_id, album_id)
);
CREATE INDEX grouping_albums_album_idx ON grouping_albums (album_id);

-- Every change, oldest first. `action`: created, edited, added, removed, from_artist (an
-- artist turned into this collective: `detail` says what moved). `track_id`/`album_id`:
-- what was added or removed.
CREATE TABLE grouping_log (
    id          uuid PRIMARY KEY DEFAULT uuidv7(),
    grouping_id uuid NOT NULL REFERENCES groupings (id),
    at          timestamptz NOT NULL DEFAULT now(),
    by_user     uuid REFERENCES users (id) ON DELETE SET NULL DEFAULT current_app_user(),
    action      text NOT NULL,
    track_id    uuid,
    album_id    uuid,
    detail      jsonb NOT NULL DEFAULT '{}',
    reason      text NOT NULL DEFAULT ''
);
CREATE INDEX grouping_log_grouping_idx ON grouping_log (grouping_id, id);

-- ---- Who sees what ----
-- A grouping, like an artist, to everyone logged in (a share's guest sees none); what's in
-- it, when they can see that track or album.
ALTER TABLE groupings ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON groupings TO acetate_app USING ((SELECT current_app_share()) IS NULL);
REVOKE DELETE ON groupings FROM acetate_app;

ALTER TABLE grouping_tracks ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON grouping_tracks TO acetate_app
    USING (EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id));

ALTER TABLE grouping_albums ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON grouping_albums TO acetate_app
    USING (EXISTS (SELECT 1 FROM albums a WHERE a.id = album_id));

ALTER TABLE grouping_log ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON grouping_log TO acetate_app
    USING (EXISTS (SELECT 1 FROM groupings g WHERE g.id = grouping_id)
           AND (track_id IS NULL OR EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id))
           AND (album_id IS NULL OR EXISTS (SELECT 1 FROM albums a WHERE a.id = album_id)));
REVOKE UPDATE, DELETE ON grouping_log FROM acetate_app;

-- ---- Sources: a grouping can have them too ----
ALTER TABLE sources ADD COLUMN grouping_id uuid REFERENCES groupings (id);
ALTER TABLE sources DROP CONSTRAINT sources_check;
ALTER TABLE sources ADD CONSTRAINT sources_check CHECK (num_nonnulls(track_id, album_id, artist_id, grouping_id) = 1);
DROP INDEX sources_external_id_key;
CREATE UNIQUE INDEX sources_external_id_key
    ON sources ((COALESCE(track_id, album_id, artist_id, grouping_id)), platform, external_id)
    WHERE external_id IS NOT NULL;
-- Like the others' (0019): a grouping's sources, by owner.
CREATE INDEX sources_grouping_idx ON sources (grouping_id) WHERE grouping_id IS NOT NULL;

DROP POLICY visible ON sources;
CREATE POLICY visible ON sources TO acetate_app USING (
    (track_id IS NOT NULL AND EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id))
    OR (album_id IS NOT NULL AND EXISTS (SELECT 1 FROM albums al WHERE al.id = album_id))
    OR (artist_id IS NOT NULL AND EXISTS (SELECT 1 FROM artists a WHERE a.id = artist_id))
    OR (grouping_id IS NOT NULL AND EXISTS (SELECT 1 FROM groupings g WHERE g.id = grouping_id))
);

-- ---- The catalog's change log (see 0029) ----
-- A track's or album's groupings are part of its row; a grouping's name and kind are in
-- the catalog's list of groupings, sent whole with every delta (`grouping` says there is
-- one to send).
ALTER TABLE library_changes DROP CONSTRAINT library_changes_kind_check;
ALTER TABLE library_changes ADD CONSTRAINT library_changes_kind_check
    CHECK (kind IN ('track', 'album', 'artist', 'annotation', 'grouping', 'everything'));

CREATE FUNCTION groupings_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    PERFORM log_change('grouping', COALESCE(NEW.id, OLD.id));
    RETURN NULL;
END
$$;
CREATE TRIGGER groupings_log_change AFTER INSERT OR UPDATE OR DELETE ON groupings
    FOR EACH ROW EXECUTE FUNCTION groupings_log_change();

CREATE FUNCTION grouping_tracks_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    PERFORM log_change('track', COALESCE(NEW.track_id, OLD.track_id));
    PERFORM log_change('grouping', COALESCE(NEW.grouping_id, OLD.grouping_id));
    RETURN NULL;
END
$$;
CREATE TRIGGER grouping_tracks_log_change AFTER INSERT OR UPDATE OR DELETE ON grouping_tracks
    FOR EACH ROW EXECUTE FUNCTION grouping_tracks_log_change();

CREATE FUNCTION grouping_albums_log_change() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public, pg_temp
AS $$
BEGIN
    PERFORM log_change('album', COALESCE(NEW.album_id, OLD.album_id));
    PERFORM log_change('grouping', COALESCE(NEW.grouping_id, OLD.grouping_id));
    RETURN NULL;
END
$$;
CREATE TRIGGER grouping_albums_log_change AFTER INSERT OR UPDATE OR DELETE ON grouping_albums
    FOR EACH ROW EXECUTE FUNCTION grouping_albums_log_change();
