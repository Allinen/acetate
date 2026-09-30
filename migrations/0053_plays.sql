-- A play log: each play of a track, with its time (annotations only keep counts and the
-- last one). Written with every play the client reports (`db::annotations::played`), and
-- brought over from Navidrome's `scrobbles` by `acetate import-navidrome` (the counts
-- come from its annotations as before; these only add when).
--
-- A play stays on the track it was of: after a merge that track redirects to the one
-- kept (`final_track`), so nothing moves and an undo has nothing to put back. Each user
-- sees only their own plays, of tracks they can see.

CREATE TABLE plays (
    id           uuid PRIMARY KEY DEFAULT uuidv7(),
    user_id      uuid NOT NULL DEFAULT current_app_user() REFERENCES users (id) ON DELETE CASCADE,
    track_id     uuid NOT NULL REFERENCES tracks (id),
    played_at    timestamptz NOT NULL DEFAULT now(),
    -- "acetate", or where it was brought over from ("navidrome").
    source       text NOT NULL DEFAULT 'acetate',
    -- For plays brought over: what they were there ("navidrome:<scrobble id>"), so running
    -- the import again adds nothing twice.
    imported_key text UNIQUE
);
CREATE INDEX plays_user_time_idx ON plays (user_id, played_at DESC);
CREATE INDEX plays_track_idx ON plays (track_id);

ALTER TABLE plays ENABLE ROW LEVEL SECURITY;
CREATE POLICY own ON plays TO acetate_app
    USING (user_id = (SELECT app_viewer()) AND EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id));
