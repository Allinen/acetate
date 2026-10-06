-- The quality a share link's copies are made at (kbps, one of the server's
-- `ACETATE_TRANSCODE_KBPS`). NULL: the server's default at the time. Doesn't matter for a
-- link that gives the original file.
ALTER TABLE shares ADD COLUMN kbps integer CHECK (kbps IS NULL OR kbps > 0);
