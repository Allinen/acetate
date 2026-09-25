-- What `files.original_filename` means: set only if the file is an *original file* (the
-- file as the artist named it, an "og file"), and then it's that name. NULL for rips,
-- downloads and everything else. It's a fact about the file, not about its tags.
--
-- Where it comes from, for now: the old tag-based library (Navidrome) kept the names of
-- original files in an ORIGINALFILENAME-like tag, so ingest reads it from there. Later,
-- clients may also give it directly when they know a file is an original.
--
-- 0012 recognized only a few spellings of that tag. Any spelling counts: case, spaces and
-- punctuation are ignored, and what's left must be "orig"/"original" + "filename"/"fname"
-- (origfilename, ORIGINAL_FILENAME, "Original File Name"...), or ID3's TOFN. Same rule as
-- `is_original_filename_key` in src/ingest/tags.rs. This fills in files 0012 left empty.

COMMENT ON COLUMN files.original_filename IS
    'Only for original files ("og files"): their name. NULL otherwise. Read from ORIGINALFILENAME-like tags for now.';

UPDATE files f SET original_filename = found.name
FROM (
    SELECT DISTINCT ON (f2.id) f2.id,
           CASE jsonb_typeof(item -> 'value')
               WHEN 'array' THEN item -> 'value' ->> 0
               ELSE item ->> 'value'
           END AS name
    FROM files f2,
         jsonb_array_elements(COALESCE(f2.embedded_tags -> 'tags', '[]')) WITH ORDINALITY AS tag(tag, tag_n),
         jsonb_array_elements(COALESCE(tag -> 'items', '[]')) WITH ORDINALITY AS item(item, item_n)
    WHERE f2.original_filename IS NULL
      AND (
          -- The key: for ID3 TXXX frames the description, for MP4 freeform atoms
          -- ("----:com.apple.iTunes:NAME") the NAME; letters and digits only, lowercased.
          lower(regexp_replace(
              CASE
                  WHEN item ->> 'key' = 'TXXX' THEN item ->> 'description'
                  WHEN item ->> 'key' LIKE '----:%' THEN regexp_replace(item ->> 'key', '^.*:', '')
                  ELSE item ->> 'key'
              END,
              '[^A-Za-z0-9]', '', 'g'))
          ~ '^(tofn|(orig|original)(filename|fname))$'
      )
      AND btrim(CASE jsonb_typeof(item -> 'value')
                    WHEN 'array' THEN item -> 'value' ->> 0
                    ELSE item ->> 'value'
                END) <> ''
    -- The first one, like ingest: tags in file order, items in tag order.
    ORDER BY f2.id, tag_n, item_n
) found
WHERE f.id = found.id;
