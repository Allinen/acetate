# Running a server

- [Users and permissions](users.md)
- [Configuration](configuration.md)
- [Command line](cli.md)

**Backups:** the library is the Postgres database plus the blob volumes. Back up both. Cover
thumbnails in the cache directory can be deleted at any time.

**HTTPS:** put a reverse proxy in front, and set `ACETATE_SECURE_COOKIES=true` and
`ACETATE_TRUST_PROXY=true` (see [Configuration](configuration.md)).

**Changing the address:** each server has an identity of its own, made once and kept in
its database (so a restored backup is the same server). The desktop app and the phone keep
their copy of the library under it, not under the address. After moving the server to a
new domain or machine, open the desktop app's **Settings → Account → Change address**: it
asks the new address which server it is, and if it's the same one (and still accepts your
login) only the address changes. Nothing is fetched again and nothing is left behind. A
different server (a new database) needs a log out and a log in, and has its own copy.

## Versions

The server and the apps each have a release (`0.1.0`) and an *API version*, a number that
goes up when a change would break an app that doesn't have it. `GET /api/version` (open to
anyone) tells the server's. When an app's differs from its server's, the desktop app says so
when it connects, under the sidebar's links (**Version mismatch**) and in **Settings →
General**: update the app if the server is newer, the server if the app is.
