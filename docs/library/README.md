# The library

How Acetate stores things, in short:

- **Files are immutable blobs.** Each file is stored once under its SHA-256:
  `data/blobs/ab/cd/abcd1234…`, no extension. The same audio on three albums is one file.
- **Metadata is in the database.** Tags are read once at import (so files don't need tags)
  and never written back. They stay viewable if you want to see them.
- **Nothing scans your folders.** Music comes in by [import or upload](ingest.md). Your old
  folder can even be mounted read-only.
- **Better copies don't overwrite.** `acetate replace-file <track> <path> --origin … --reason
  "flac upgrade"` swaps in a higher quality file. The old one stays until an admin purges it.
- **Original ("OG") files** are real studio originals, marked by someone who knows. Lossless
  rips don't count.

Parts:

- [Importing music](ingest.md)
- [Moving from Navidrome](navidrome.md)
- [Storage, trash and purging](storage.md)
