-- Music videos are extras (kind "video", "live video"…): their files get what a video
-- has (size, frame rate, codec) and a thumbnail (a frame, stored as an image; served like
-- covers, to those who see the extra). `imported_from`: the path a file came from when the
-- server imported it from disk (`acetate videos`). See `db::extras`, `ingest::video`.

ALTER TABLE extra_files
    ADD COLUMN width         integer,
    ADD COLUMN height        integer,
    ADD COLUMN frame_rate    real,
    ADD COLUMN video_codec   text,
    ADD COLUMN thumb_sha256  text REFERENCES blobs (sha256),
    ADD COLUMN imported_from text;

CREATE INDEX extra_files_thumb_idx ON extra_files (thumb_sha256) WHERE thumb_sha256 IS NOT NULL;
