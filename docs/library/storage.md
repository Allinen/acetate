# Storage, trash and purging

## Volumes

Blobs can be spread over several disks, for example a fast SSD for most music and a big HDD
for the less important part. `ACETATE_VOLUMES=main=/srv/acetate,archive=/mnt/hdd/acetate`
names them (each gets `blobs/` and `tmp/`); the first is the default, and where covers go.
Without it there is one volume, `main`, in `ACETATE_DATA_DIR` (default `./data`).
`--volume` on ingest picks one. Admins can see what each volume holds in Settings → Server.

Imports from the same filesystem are reflinks: instant and free of extra space.

The desktop app can keep a [mirror](mirror.md) of the blobs, per volume.

## Trash

**Nothing is deleted directly.** Covers (need `edit_metadata`), sources (`edit_sources`),
old audio files (admins only) and playlists go to the trash first, and can be taken back
out. Open it from the command palette ("Trash").

*Old files* are files no track plays anymore (replaced by a better one, or left from a
merge), listed with their sizes: this is where merges and upgrades free space.

## Purge

Purging is the only deletion, and it is deliberate:

- admins only;
- only things at least **7 days** in the trash;
- with a typed confirmation, checked by the server.

Details:

- Purging a track's file deletes its bytes but keeps its row as a tombstone: its tags,
  origin and place in the history stay.
- A blob is deleted only when nothing uses it anymore (a copied cover shares its blob),
  and only after the database change has committed. A crash can leave a stray blob, never a
  row whose blob is missing.
- Every purge is logged (who, when, what, bytes freed), readable by admins.

## Maintenance commands

`acetate measure-covers` fills in cover sizes and blob sizes (lists first, `--apply` to do
it). `acetate verify --blobs` checks the blob store. See [Command line](../admin/cli.md).
