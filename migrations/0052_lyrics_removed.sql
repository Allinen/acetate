-- Lyrics can be removed (one added to the wrong track, or by mistake): a revision marks
-- the entry removed (`removed` in its data, kept here for queries), with its text kept.
-- Restoring an earlier revision brings it back. Removed entries aren't shown with the
-- track's lyrics, only listed apart (`removed_lyrics`) for taking back.
ALTER TABLE lyrics ADD COLUMN removed boolean NOT NULL DEFAULT false;
