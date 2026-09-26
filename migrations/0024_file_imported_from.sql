-- Where a file was imported from: its path on the machine that sent it (the CLI's disk,
-- or what an HTTP client says). NULL when unknown or meaningless (an upload from a
-- browser, a download that only ever lived in memory). A record of where things came
-- from, and a fast way to find a file again by path (e.g. matching Navidrome's files
-- without hashing them). Set at import; an import that finds the file already stored
-- fills it in if it's still NULL. Otherwise files stay immutable (see 0008 and 0009).
ALTER TABLE files ADD COLUMN imported_from text;
CREATE INDEX files_imported_from_idx ON files (imported_from) WHERE imported_from IS NOT NULL;
