-- What each user has done with a track, album or artist: how often they played it and
-- when last, their rating, whether they starred it. Like Navidrome's `annotation` table
-- (and imported from it, see src/navidrome.rs).
--
-- Personal: a user sees and changes only their own, and only on things they can see.

CREATE TABLE annotations (
    id             uuid PRIMARY KEY DEFAULT uuidv7(),
    user_id        uuid NOT NULL DEFAULT current_app_user() REFERENCES users (id) ON DELETE CASCADE,
    track_id       uuid REFERENCES tracks (id),
    album_id       uuid REFERENCES albums (id),
    artist_id      uuid REFERENCES artists (id),
    play_count     integer NOT NULL DEFAULT 0 CHECK (play_count >= 0),
    last_played_at timestamptz,
    -- 0 to 10, shown as 0 to 5 stars in half steps. 0 = not rated.
    rating         smallint NOT NULL DEFAULT 0 CHECK (rating BETWEEN 0 AND 10),
    rated_at       timestamptz,
    starred        boolean NOT NULL DEFAULT false,
    starred_at     timestamptz,
    CHECK (num_nonnulls(track_id, album_id, artist_id) = 1)
);

-- One row per user and thing. (Ids are UUIDv7 in every table, so they don't collide
-- across tracks, albums and artists; `sources` is keyed the same way.)
CREATE UNIQUE INDEX annotations_user_item_key
    ON annotations (user_id, (COALESCE(track_id, album_id, artist_id)));
CREATE INDEX annotations_track_idx ON annotations (track_id) WHERE track_id IS NOT NULL;
CREATE INDEX annotations_album_idx ON annotations (album_id) WHERE album_id IS NOT NULL;
CREATE INDEX annotations_artist_idx ON annotations (artist_id) WHERE artist_id IS NOT NULL;

ALTER TABLE annotations ENABLE ROW LEVEL SECURITY;
CREATE POLICY own ON annotations TO acetate_app
    USING (
        user_id = (SELECT app_viewer())
        AND (EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id)
             OR EXISTS (SELECT 1 FROM albums a WHERE a.id = album_id)
             OR EXISTS (SELECT 1 FROM artists a WHERE a.id = artist_id))
    );
