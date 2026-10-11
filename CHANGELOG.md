# Changelog

## 0.2.0 - 2026-10-12

**Upgrading:** the server and the apps now have to be the same version (see below), so update
the server first and then the apps; the desktop app offers to update itself when it finds the
server newer. Back up the database before updating: a new migration moves the vocabulary into
one table. 

### Added

- **Extensions.** What Acetate knows about a place music comes from (YouTube, SoundCloud,
  NetEase, QQ, Kuwo, stores like Qobuz and Tidal) is now data in `extension.toml` files that the
  server loads and every app reads: the fields of a platform's sources, how a pasted link
  becomes a source, how a link in a file's comment counts. Platforms of your own are added
  with a file, with no rebuild. YouTube's and SoundCloud's metadata reading are small Python
  programs the server runs. The Python library can use such platforms too.
- **Vocabulary, editable.** Statuses, credit roles, album kinds, relations, review flag kinds,
  kinds of extra and the words that make a title another version of a song are one table an
  admin changes in Settings → Vocabulary: add, rename (everything that uses the word follows),
  describe, remove when unused. The words the app is built around (`primary`, `featured`,
  `released`...) are locked.
- **Making tracks of extras goes through the track editor.** Many extras at once, each a draft
  from its YouTube video, edited together like files that didn't import. There are other enhancements
  on unlinked extras and converting extras to tracks too.
- **The app and the server must be the same version.** A server turns away an app of another API
  version or catalog format (426); the desktop app shows an update screen instead of its pages,
  and the phone's version dialog can't be put off.
- **Android:** the equalizer.
- **YouTube** cookies for "confirm you're not a bot": `ACETATE_YTDLP_COOKIES`.
- The user guide is served by the server at `/docs`.

### Changed

- The web interface is gone. The server keeps the page for share links and the page for links to
  tracks, albums and artists. Share links still play in a browser.
- The catalog's ETag is its version, so an unchanged library answers without building anything.
- A secret setting like `ACETATE_SECURE_COOKIES` must be a real boolean; a typo stops the server
  instead of turning it off.
- Thumbnails of videos are only fetched from the platform's image hosts, and a missing one says why.
- One place for sizes, counts and fonts across the apps.

### Removed

- The one-off repair commands `merge-duplicates`, `recheck-duplicates`, `duplicate-albums`,
  `fix-merged-covers`, `measure-covers`, `own-album-covers` and `producers-from-composer`,
  and the endpoints `GET /api/tracks`, `/api/search` and `/api/library/albums`.
- The folder rule that read `Singles` and `Albums` folders as a hint.

### Fixed

- Search ignores punctuation-only words ("Artist - Song") and no longer matches letters scattered
  over a long title.
- A track made from an extra is no longer marked an original file, and can be undone from the
  extra's page.

## 0.1.0 - 2026-10-10

The first public release.

- The server (with Postgres) and its command line, as a Docker image for amd64 and arm64
  at `ghcr.io/allinen/acetate`.
- The desktop app for Linux and Windows, and the Android app.
- Basically everything else too.
- The user guide: https://allinen.github.io/acetate/
