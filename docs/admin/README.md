# Running a server

- [Users and permissions](users.md)
- [Configuration](configuration.md)
- [Command line](cli.md)

**Backups:** the library is the Postgres database plus the blob volumes. Back up both. Cover
thumbnails in the cache directory can be deleted at any time.

**HTTPS:** put a reverse proxy in front, and set `ACETATE_SECURE_COOKIES=true` and
`ACETATE_TRUST_PROXY=true` (see [Configuration](configuration.md)).
