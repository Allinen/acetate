-- A file's original filename comes only from its own tags (ORIGINALFILENAME, TOFN and
-- their spellings, see `find_original_filename` in src/ingest/tags.rs). NULL when the tags
-- don't have one. It marks "original files" (the file as the artist named it), as opposed
-- to rips and downloads, so it must never be filled in from anything else. The name a file
-- was uploaded with stays in `uploaded_filename`.

ALTER TABLE files ALTER COLUMN original_filename DROP NOT NULL;

COMMENT ON COLUMN files.original_filename IS
    'From the file''s own tags (ORIGINALFILENAME, TOFN...) only; NULL if they have none.';

-- Until now, files without such a tag got their uploaded name here. Clear those: the same
-- rules as `find_original_filename`, applied to the raw tags stored with each file (key
-- compared ignoring case; for ID3 TXXX frames the description is the key; for MP4
-- freeform atoms "----:com.apple.iTunes:NAME" it's NAME; the value must not be blank).
UPDATE files f SET original_filename = NULL
WHERE NOT EXISTS (
    SELECT 1
    FROM jsonb_array_elements(COALESCE(f.embedded_tags -> 'tags', '[]')) tag,
         jsonb_array_elements(COALESCE(tag -> 'items', '[]')) item
    WHERE upper(CASE
                    WHEN item ->> 'key' = 'TXXX' THEN item ->> 'description'
                    WHEN item ->> 'key' LIKE '----:%' THEN regexp_replace(item ->> 'key', '^.*:', '')
                    ELSE item ->> 'key'
                END) IN ('ORIGINALFILENAME', 'ORIGINAL_FILENAME', 'ORIGINAL FILENAME', 'TOFN')
      AND btrim(CASE jsonb_typeof(item -> 'value')
                    WHEN 'array' THEN item -> 'value' ->> 0
                    ELSE item ->> 'value'
                END) <> ''
);
