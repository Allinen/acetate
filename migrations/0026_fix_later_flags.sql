-- "Fix later": an import can put every track it makes on the Review page with a note
-- (`acetate ingest --flag "<note>"`, descriptions' `flag`), for libraries imported quickly
-- with poor metadata, to go through and fix afterwards. `related` holds the note.
ALTER TABLE review_flags DROP CONSTRAINT review_flags_kind_check;
ALTER TABLE review_flags ADD CONSTRAINT review_flags_kind_check
    CHECK (kind IN ('possible_duplicate', 'ambiguous_artist', 'merged_artist', 'fix_later'));
