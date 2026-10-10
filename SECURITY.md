# Security

Acetate is a personal project, developed by Allinen and run on my own server. Reports
are read and acted on as time allows (no promises about how fast), and a fix goes into the
next release.

## Reporting a problem

Please **don't open a public issue** for a security problem. Use GitHub's private
reporting instead: the repository's **Security** tab -> **Report a vulnerability**. Say what
you found, how to reproduce it, and what version (`GET /api/version`, or Settings in an
app). You'll get a reply there.

## What counts

Acetate is meant to be reachable from the internet, so these are the things that matter:

- getting in without logging in, or getting more than a user's permissions allow
  (see [Users and permissions](docs/admin/users.md));
- seeing or playing tracks restricted to groups you aren't in (restricted tracks are hidden
  by the database itself, via row-level security);
- reading or writing files outside the library and the data folder (path handling in
  uploads, ingest, the mirror and downloads);
- making the server fetch addresses it shouldn't (for example the metadata importers);
- anything in an upload (tags, covers, videos, lyrics) that crashes the server, runs code,
  or fills the disk past the configured limits.

Things that aren't: a server run over plain HTTP on the open internet (use HTTPS with a
reverse proxy; see Configuration), a weak password a user chose, or an admin doing what
admins may do.

## Supported versions

Only the latest release gets fixes. The app and the server are released together; update
both.
