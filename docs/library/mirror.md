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

## Checking files

- A download is hashed as it comes in (that costs nothing extra), and only kept if its hash
  is its name. One that isn't is thrown away and fetched again.
- A download that stopped goes on from where it was the next time (and what it had is
  hashed first).
- Files taken in from your own folders (below) are trusted by their path and size, unless
  you tick **Hash files from these folders before taking them in** (slow: it reads every
  file). Settings shows how many weren't hashed yet.
- **Hash every file again** checks every file here, those too. One that isn't what its name
  says is set aside in the `corrupt` folder (never deleted) and fetched again.
- Files already in the mirror's folder that it didn't put there (a copy of a server volume,
  say) count if they're the size the server lists them with (or are hashed, with that box
  ticked).
- **Stop** stops what's running (between files, or mid-file when hashing or copying);
  **Sync now** goes on.

## Files you already have

If the music the library was imported from is on this computer, add its folder under
**Look in**. The server remembers where it imported each file from (`/srv/music/Artist/a.flac`),
so the folder is paired with the server's folder it has the files of: the app works that
out when you add it (how many of its files are at the same paths, with the same sizes) and
shows it to change or accept.

Before downloading, the mirror then looks for each missing file at its path in your folder
(`/srv/music/Artist/a.flac` → `<your folder>/Artist/a.flac`), and takes it in if it's there
with the right size: as a reflink where the disk can (btrfs, XFS, ZFS with block cloning:
no extra space, and changing your file later never changes the mirror's), else as a copy.
Nothing is read to find them, so it's quick. Your files are only read, never changed.
Covers and extras have no such path: they're downloaded.

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
mirror. **Settings, This computer** shows it, and **Move this folder…** moves
it, for example to a bigger disk: it says what would move and how much first. On the same
disk it's instant; to another disk everything is copied, each file's hash checked, and only
then removed from the old place. The app starts again from the new folder. Scaled covers
stay in the system's cache folder.

## Without the window

The same mirror runs from the command line, with the app's folder and login:

```sh
acetate-desktop mirror status      # what's mirrored, missing and removable
acetate-desktop mirror match /music  # which of the server's folders it has the files of
acetate-desktop mirror set enabled=yes volumes=main look-in=/music=/srv/music
acetate-desktop mirror sync        # bring it up to date once
acetate-desktop mirror watch       # keep it up to date (what the service runs)
acetate-desktop mirror rehash
acetate-desktop mirror removable
acetate-desktop mirror remove all
```

`ACETATE_ROOT` picks another folder.
