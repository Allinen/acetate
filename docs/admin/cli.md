# Command line

`acetate <command> --help` has the full details. These talk to the database directly, so they
act as the system with no login. Many one-off tools list what they would do unless you pass
`--apply` (or `--dry-run`).

| Command | Does |
|---|---|
| `serve` | run the HTTP server (migrations run first) |
| `user create\|set-password\|set-groups\|list` | [manage users](users.md) |
| `ingest <dir> --layout …` | [import a folder](../library/ingest.md) (videos in it as extras) |
| `ingest <dir> --videos` | [only the videos in a folder](../library/ingest.md#videos), as extras |
| `import --origin … <files>` | import individual files |
| `replace-file <track> <path> --origin …` | swap in a better file; the old one stays |
| `import-navidrome --db …` | [users, plays, stars, ratings from Navidrome](../library/navidrome.md) |
| `verify [dir] [--blobs]` | check files are safely stored (read-only) |
| `review-checks [--on x\|--off x]` | list or toggle the [review checks](../review.md) |
