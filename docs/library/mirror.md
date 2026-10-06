# A mirror on your computer

The desktop app can keep a copy of the server's files on your computer: a second copy of
the library, and the start of offline playback. It's for admins: it's every file, hidden
tracks' too. It's off until you turn it on, in **Settings, Mirror**.

## What it keeps

- **Audio, covers, extras, and files that didn't import** (they're bound to end up in the
  library), on the volumes you tick. Each volume is a choice of its own: an archive volume
  you care less about can stay on the server only. Each shows how many files it has and
  their size before you tick it.
- **Previous files** (tracks' earlier files, replaced by better ones) only if you ask for
  them: they can be many, and aren't played.

The files are kept as the server keeps them: `blobs/ab/cd/abcd1234…`, by their SHA-256,
with no extension and no tags written in. Not a folder of tagged copies.

## Every file is checked

- A file is hashed as it downloads, and only kept if its hash is its name. One that isn't
  is thrown away and fetched again.
- A download that stopped goes on from where it was the next time (and what it had is
  hashed first).
- **Hash every file again** checks every file here. One that changed on disk is set aside
  in the `corrupt` folder (never deleted) and fetched again.
- Files already in the mirror's folder that it didn't put there (a copy of a server volume,
  say) are hashed before they count.

## Files you already have

If the music the library was imported from is on this computer, add its folder under
**Look in**. Before downloading, the mirror hashes the files there that have the size of a
missing one, and takes in those that match: as reflinks where the disk can (btrfs, XFS,
ZFS with block cloning: no extra space, and changing your file later never changes the
mirror's), else as copies. Your files are only read. A file hashed once isn't hashed again
unless it changes.

## Keeping up

While the app is open, the mirror keeps up as the server gets files: the server tells it
of a change as it happens, so a new import is here within seconds.

**Set up the mirror service** (in the same section) keeps it up while the app is closed
too: a systemd user service on Linux, a task run at logon on Windows, a launch agent on
macOS. When the app opens, the service steps aside and the app takes over; when it closes,
the service goes on. It runs this app's program where it is now: install it again after
moving the app.

On Linux a user service runs while you're logged in; `loginctl enable-linger` keeps it
running after you log out.

## Nothing is removed unasked

When the server lets a file go (trashed, purged, replaced), or you stop mirroring a volume,
the mirror keeps it and lists it under **Nothing wants these anymore**, grouped by why.
You remove a group when you choose to; files the server no longer has say so, as they
can't be fetched again.

## Where it is

Everything the app keeps is in one folder: settings, the library, playlists, and the
mirror. **Settings, Library on this computer** shows it, and **Move this folder…** moves
it, for example to a bigger disk: it says what would move and how much first. On the same
disk it's instant; to another disk everything is copied, each file's hash checked, and only
then removed from the old place. The app starts again from the new folder. Scaled covers
stay in the system's cache folder.

## Without the window

The same mirror runs from the command line, with the app's folder and login:

```sh
acetate-desktop mirror status      # what's mirrored, missing and removable
acetate-desktop mirror set enabled=yes volumes=main look-in=/music
acetate-desktop mirror sync        # bring it up to date once
acetate-desktop mirror watch       # keep it up to date (what the service runs)
acetate-desktop mirror rehash
acetate-desktop mirror removable
acetate-desktop mirror remove all
```

`ACETATE_ROOT` picks another folder.
