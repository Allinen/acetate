# Acetate

Acetate is a self-hosted music server intended for archivists, metadata fanatics, and multi-user setups. In Acetate, your tags don't matter - **the database has everything**. Audio files
are stored once, untouched, and never rewritten. Everything else (titles, artists, albums,
covers, credits, notes, where a file came from) lives in the database, where it can be
edited instantly, in bulk, and undone.

These pages are the user guide. If you want the pitch, see the
[README](https://github.com/Allinen/acetate#readme).

## Where to start

- **Setting up a server:** [Getting started](getting-started.md), then
  [Importing music](library/ingest.md).
- **Coming from Navidrome:** [Moving from Navidrome](library/navidrome.md).
- **Using the desktop app:** [Listening](listening.md) and the [keys](keys.md). Press `?`
  in the app to see every key.
- **Fixing your library's metadata:** [Editing](editing/README.md) and [Review](review.md).

## Ideas worth knowing first

- **Files are immutable blobs.** A file is stored under its SHA-256. Tags are read once at
  import and never written back. Nothing here edits, renames or deletes your music.
- **Two modes.** Normally you *listen*: nothing can change by accident. Press `e` for
  *edit mode*, where the editing tools appear.
- **Everything has history.** Almost every change is logged with who made it, and can be
  restored.
- **Nothing is deleted directly.** Things go to the [trash](library/storage.md) first;
  only an admin can purge, and only after a week.
- **The library lives on your computer too.** The desktop app keeps the whole catalog in
  memory, so searching and browsing are instant and work offline. Offline listening is not yet implemented
- **Keyboard shortcuts are worth learning.** Mouse works too, but every page is built around keys.
