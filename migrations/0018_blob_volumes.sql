-- Blobs can live on different volumes (disks): a fast one for most music, a big slow one
-- for the rest, chosen per import (see `storage::Volume`). This records which volume each
-- blob is on, by name; where a volume is on disk is each machine's configuration
-- (ACETATE_VOLUMES), so a dump restored elsewhere only needs that machine's paths. The
-- server checks at startup that every volume named here is configured.
--
-- Reading doesn't need this (a blob is found by looking on each volume); it's the record
-- for accounting, checks (`acetate verify --blobs`) and moving blobs between volumes.

CREATE TABLE blobs (
    sha256     text PRIMARY KEY CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    volume     text NOT NULL CHECK (volume ~ '^[a-z0-9_-]+$'),
    size_bytes bigint,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX blobs_volume_idx ON blobs (volume);

-- Everything stored so far is on the one volume there was.
INSERT INTO blobs (sha256, volume, size_bytes)
SELECT sha256, 'main', size_bytes FROM files WHERE purged_at IS NULL
ON CONFLICT DO NOTHING;
INSERT INTO blobs (sha256, volume)
SELECT DISTINCT blob_sha256, 'main' FROM covers
ON CONFLICT DO NOTHING;
INSERT INTO blobs (sha256, volume, size_bytes)
SELECT sha256, 'main', size_bytes FROM ingest_problems WHERE resolved_at IS NULL
ON CONFLICT DO NOTHING;

-- Only the system (ingest, purge, the CLI) reads or writes this: it's about disks, and it
-- would tell users how many files exist, hidden ones included.
ALTER TABLE blobs ENABLE ROW LEVEL SECURITY;
REVOKE ALL ON blobs FROM acetate_app;
