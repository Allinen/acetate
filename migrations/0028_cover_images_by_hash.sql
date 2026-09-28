-- Cover images by their content (`GET /api/images/{hash}`, also by the hash's start, as
-- the catalog's `cover_key` gives it): the same image under many covers (every track of
-- an album often has its own copy) is fetched and cached once. `text_pattern_ops` lets a
-- prefix (`LIKE 'abc%'`) use the index.
CREATE INDEX covers_blob_sha256_idx ON covers (blob_sha256 text_pattern_ops);
