-- Images kept as an extra's files: a `width` and no `video_codec` (a video has both). They
-- are served by `GET /api/images/{hash}` like covers, looked up by the start of their hash.
CREATE INDEX extra_files_image_idx ON extra_files (sha256 text_pattern_ops)
    WHERE width IS NOT NULL AND video_codec IS NULL;
