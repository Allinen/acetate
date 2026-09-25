-- Merging artists can flag the tracks whose credits it moved, so a person can check them
-- (e.g. after merging "A feat. B" into A, those tracks may need B added back as featured).
-- related = {"from": id, "from_name": "...", "into": id, "artists": [from, into]}.
ALTER TABLE review_flags DROP CONSTRAINT review_flags_kind_check;
ALTER TABLE review_flags ADD CONSTRAINT review_flags_kind_check
    CHECK (kind IN ('possible_duplicate', 'ambiguous_artist', 'merged_artist'));
