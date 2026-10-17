	# Changelog

## 0.4.3 - 2026-10-17

- Fix CI migrations

## 0.4.2 - 2026-10-17

- Fix CI test race condition

## 0.4.1 - 2026-10-17

### Fixes

- Playing something in a new, empty listening session now shares it; before, only changes to a session that already had a track were sent

## 0.4.0 - 2026-10-17

### Features

- The desktop track page always shows the track's status, including released, not only the unusual ones
- Listen together: invite-only shared sessions that keep everyone's player in step, and an opt-in list of what others are listening to
- Ctrl+Enter plays the selected album in grids, and release cards on artist pages get the same hover play button as the Albums grid
- Color themes on desktop: six built-in palettes and user-made ones (a copy of the theme in use with its colors changed), chosen in Settings → Appearance and applied live
- Android settings are a page with Sound, Downloads and Account tabs instead of one long dialog
- Color themes on Android, with the palettes moved to the shared core so both clients have the same ones; Settings → Appearance picks one or edits your own

### Fixes

- Esc in the command palette only closes it instead of also going back a page; Backspace on its empty line closes it
- The theme color pickers choose opaque colors, without the opacity and blending options
- The lowest spectrum bars no longer move in lockstep; the FFT window is doubled and bars narrower than one bin interpolate between bins

## 0.3.1 - 2026-10-13

### Features

- With shuffle on, a Play button on a playlist, album or other track list starts at a random track
- The desktop app offers to update to the server's release from the sidebar instead of showing a version-mismatch label; a server older than the app is only warned about
- The Android app updates itself to the server's newer release: the core downloads the APK and checks its checksum (shared with the desktop), the system installer applies it

## 0.3.0 - 2026-10-12

### Changes

- More popups and pages poll a Task instead of an Arc<Mutex<Option>>
- The settings tabs and the edit forms poll a Task (or a Fetched value) instead of an Arc<Mutex<Option>>
- The import pages, the app's share link and the editor's lookups poll a Task
- The catalog's credits, memberships and track/album lookups are flat columns kept in the snapshot instead of rebuilt and held on the heap

### Features

- The server logs in as the app role itself; as_system switches to a limited system role instead of the owner
- Settings → Users in the desktop app: people and their groups, groups and their permissions (GET /api/permissions lists them)
- Health checks on the blob store, run from Settings → Health (or acetate verify --blobs); report-only, new checks are added in one place
- The album page in edit mode lists title, kind, date and artists as rows with the track editor's keys, and changes go through the commandline; the click-to-pick form is gone
- Credited as… has a commandline and Alt+C, and an alias that is still credited opens it with its credits chosen
- A bar starts a fresh command in every commandline (no leftover choices), digits and Backspace first in an empty commandline are the app's keys, Ctrl+7 is Liked songs, Credited as… sets the artist while the line is typed, -alias:Name recredit:yes, and Credited as… moves to Ctrl+B
- Credited as… lists its keys when nothing is typed or a value is done (Tab leaves a space, not the next key), a bar brings the commands back in every commandline, and Liked songs shows its 7 in the sidebar
- A video made into a track plays its sound, copied out of the video as it is, and the video stays an extra linked to the track; Health finds tracks made earlier that play a video file and can take the sound out of them
- Large commandline rework. Commandlines can be declared once as a schema that drives parsing, the choice list and the checks; Credited as… is the first, and takes several artists at once
- The artist and album page commandlines are declared as schemas, with places as n: instead of #N
- The track editor's and track page's commandline is declared as a schema, with places as n: and values instead of #N
- Searches, the palette and the share popup are declared as schemas too, and the old Keys parser is gone
- The commands of every commandline are documented from their declarations, with graphs, and the keys overlay has a Commands tab
- The sidebar's links sit on two rows at 14 pt, Changes, Settings and Trash above Keys and Log out

### Fixes

- Comparing two tracks from the middle shows the seeker there, not at the start
- Credited as… keeps listing a name's choices after a space inside it; TODO: unique artist labels and one completion model
- An album can credit one artist in several roles, and its staged changes show right under the commandline
- The user guide opens at /docs as well as /docs/, and the image builds the book with its command graphs
- The sidebar's Settings, Trash, Keys and Log out are a little smaller, clear of the pane's edge; the command graphs are dropped from the docs

## 0.2.1 - 2026-10-12

0.2.0 was tagged but its desktop packages failed to build (a moved font licence), so nothing was
published under it: this is 0.2.0 with the fix and what came after. Read 0.2.0's notes below too.
The API version is 7, so apps and server have to be updated together (the desktop app offers to
update itself; the phone's dialog links the APK).

### Added

- **Settings → Account (desktop)** has what the web interface used to: change your password, see
  the devices you're logged in on and log any of them out, and list and revoke your share links.
- **Album covers are a draft (desktop).** Making a cover main, moving, trashing, copying and adding
  images, and putting one on all the tracks are kept until you press Save (or Discard), as on the
  phone.

### Changed

- Logging out now ends the session on the server too, instead of only forgetting it here.
- A catalog update sends the playlists and groupings only when one changed, not every time.

### Fixed

- The Linux and Windows packages build again.

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
