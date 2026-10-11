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

The server and the apps each have a release (`0.1.0`), an *API version* and a *catalog
format*: numbers that go up when a change would break an app that doesn't have it. The server
and an app **have to be the same** in both. The apps send theirs with every request
(`X-Acetate-Api`, `X-Acetate-Catalog`); a server of other numbers answers **426 Upgrade
Required** and says so, and an app that finds out (`GET /api/version`, open to anyone, says the
server's) stops and shows what to do instead of its pages:

* **the server is newer**: the desktop app offers **Update to {the server's release}** (it
  downloads that release from GitHub and puts it in place; it starts the next time Acetate
  does). The phone app links to the APK of the server's release.
* **the app is newer** (the server wasn't updated yet): it says to update the server to the
  app's release, and links the app that goes with the server, to install by hand: the apps
  never go back to an older release by themselves, since what they keep on the device may not
  be readable by one.

A different release with the same two numbers (`0.1.1` against `0.1.0`) works, with a note that
the versions differ. Scripts (curl, the Python library) and the web pages send no numbers and
aren't turned away.
