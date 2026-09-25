-- Album kinds are kinds of *release*: album, EP, single, compilation, mixtape. An era or a
-- project groups releases and tracks; it isn't a release (no track numbers, no album
-- credits of its own), so it'll be its own feature. None exist yet; if some album used
-- these kinds, the foreign key would stop this migration.
DELETE FROM album_kinds WHERE kind IN ('era', 'project');
