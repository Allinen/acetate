-- A share link plays a transcoded copy of its tracks (`storage::transcode`), unless its
-- creator chose to give the original file: only possible with `download_original`, and
-- the link falls back to copies if its creator loses that permission (like losing `share`
-- ends a link: a share looks through its creator's eyes, 0048).
ALTER TABLE shares ADD COLUMN original boolean NOT NULL DEFAULT false;
