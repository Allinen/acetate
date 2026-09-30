-- Notes: longer free text about a track, an album or an artist (what's known about it,
-- where it came from, stories), shared like the rest of the library. Any number per item,
-- each with a title. Every change is a revision (`note_revisions`), and a note is never
-- deleted: taking it off hides it, and it can be brought back. Notes stay with what they
-- were written on; a page also shows the notes of what was merged into it (so undoing a
-- merge needs nothing here). See `db::notes`.

CREATE TABLE notes (
    id         uuid PRIMARY KEY DEFAULT uuidv7(),
    track_id   uuid REFERENCES tracks (id),
    album_id   uuid REFERENCES albums (id),
    artist_id  uuid REFERENCES artists (id),
    CHECK (num_nonnulls(track_id, album_id, artist_id) = 1),
    title      text NOT NULL DEFAULT '',
    body       text NOT NULL DEFAULT '',
    -- Taken off (hidden, not deleted).
    hidden_at  timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by uuid REFERENCES users (id) ON DELETE SET NULL DEFAULT current_app_user()
);

CREATE INDEX notes_track_idx ON notes (track_id) WHERE track_id IS NOT NULL;
CREATE INDEX notes_album_idx ON notes (album_id) WHERE album_id IS NOT NULL;
CREATE INDEX notes_artist_idx ON notes (artist_id) WHERE artist_id IS NOT NULL;

CREATE TABLE note_revisions (
    id         uuid PRIMARY KEY DEFAULT uuidv7(),
    note_id    uuid NOT NULL REFERENCES notes (id),
    title      text NOT NULL,
    body       text NOT NULL,
    hidden     boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by uuid REFERENCES users (id) ON DELETE SET NULL DEFAULT current_app_user(),
    reason     text NOT NULL DEFAULT ''
);

CREATE INDEX note_revisions_note_idx ON note_revisions (note_id, id DESC);

-- Seen as far as what they're about is.
ALTER TABLE notes ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON notes TO acetate_app USING (
    (track_id IS NOT NULL AND EXISTS (SELECT 1 FROM tracks t WHERE t.id = track_id))
    OR (album_id IS NOT NULL AND EXISTS (SELECT 1 FROM albums al WHERE al.id = album_id))
    OR (artist_id IS NOT NULL AND EXISTS (SELECT 1 FROM artists a WHERE a.id = artist_id))
);
ALTER TABLE note_revisions ENABLE ROW LEVEL SECURITY;
CREATE POLICY visible ON note_revisions TO acetate_app
    USING (EXISTS (SELECT 1 FROM notes n WHERE n.id = note_id));

-- Never deleted; a revision is never rewritten.
REVOKE DELETE ON notes FROM acetate_app;
REVOKE UPDATE, DELETE ON note_revisions FROM acetate_app;
