-- A file that didn't import keeps everything it was imported with, not just its origin:
-- the status, the groups it was to be restricted to, the platform ids of its artists and
-- album, its album position. So importing it later from the Review page gives the same
-- result as if it had imported the first time (and a restricted track doesn't come out
-- public). Stored like `POST /api/ingest`'s description (see `NewDescription`), minus
-- album cover images.
--
-- Also the title and artists its tags did have, to fill in the form for what's missing.

ALTER TABLE ingest_problems
    ADD COLUMN description jsonb,
    ADD COLUMN tag_title   text,
    ADD COLUMN tag_artists text[] NOT NULL DEFAULT '{}';

UPDATE ingest_problems SET description = jsonb_build_object('origin', origin) WHERE origin IS NOT NULL;

ALTER TABLE ingest_problems DROP COLUMN origin;
