# The track page

A track's page has a **commandline**, like the [artist page](artist-page.md) and the
[album page](album-page.md). It's the [track editor](editor.md)'s commandline: one
language, `key:value` with `|` between commands, the editor's keys (`title:`, `artist:`,
`cover:`, `source:`…) and the page's own.

In listen mode the commandline sits under the header, on its own, for listening and moving
around. In edit mode it's the editor's commandline, with the editor's fields and preview
under it. It has the cursor when the page opens, and `/` gets back to it. `Alt+E` (or the
word `edit`) turns edit mode on or off without leaving it.

While you type, the choices appear under the commandline. `↑`/`↓` choose, `Tab` takes the
choice, and `Enter` takes it and does the command. Plain words find things by name: `next`
finds *Play next*, `ann` finds the track's artist Ann (to go to the artist's page), `video` finds
*Watch the video*. If you're not sure what's there, type a word.

## Listening

```
play:
queue:next | star:
rating:4.5
```

- `play:` plays the track, then the rest of its album.
- `queue:` puts it at the end of the queue, `queue:next` before what's queued.
- `star:` stars it, or unstars it if it's starred (`star:yes`, `star:no` and `-star` say
  which). `rating:3.5` is your rating, in half stars; `-rating` clears it.
- *Copy link*, *Copy a share link*, *Watch the video* and *Cover in full* (as words) do
  what they say.

## Moving around

- `lyrics`, `credits`, `file`, `notes`, `extras`, `versions`, `tags` and `history` (as
  `key:`, or as words) open that part of the page and scroll to it. A part that's already
  open is closed instead, and the list says which it will do. *Related tracks*, *Sources* and
  *Covers* are found by words (`related:`, `source:` and `cover:` are the editor's keys). In
  edit mode, `merges` goes to the merges that can be undone, and *Edit lyrics* puts the
  cursor in the lyrics' text.
- `sync` (edit mode) opens the lyrics' form with the timing popup on (a new entry, if the track has no lyrics).
- `lyrics:English` shows that entry of its lyrics (or `lyrics:#2`, the second one).
- The words of an artist's name, an album's title, a grouping's or a related track's go to
  its page.

## Changing the track (edit mode)

The editor's keys change the track's metadata, covers and sources: see
[the track editor](editor.md). The page adds these. They're staged as chips with the
editor's changes, and `Ctrl+Enter` saves them with the rest. Each of the page's is saved on
its own: if one fails, it stays staged and says why, and the rest are still saved. Outside
edit mode they aren't suggested, and if you type one anyway it tells you why it can't be
done.

- `note:`, `-note:`, `extra:` and `-extra:` work as on the [artist page](artist-page.md).
- `og:` says the track's file is an original file, named as it was uploaded; `og:Name`
  gives it another name, and `-og` says it isn't one. Only do this if you know where the file
  came from. Needs `edit_files`.
- `file:#2` plays its second earlier file again (the list numbers them); the file it plays
  now becomes an earlier one. Needs `replace_file`.
- `unmerge:` undoes the newest merge into it: the merged track is its own again, as it was
  before. Needs `replace_file`.
- `restore:#3` restores its third revision (the list has them, newest first), as a new
  revision. Needs `restore_revision`.
- *Duplicate this track* (as words) stages a copy of it; its page opens once it's made.
  *Undo the last origin change* puts back where its file came from.
- *Merge with another track…* and *Turn into an extra…* (as words) open those tools.

## Versions

```
version:Leak title
version:Leak title as:Leaked
```

- `version:Name` edits that version, or makes a new version by that name (a copy of the one
  being edited) and edits it.
- `version:Name as:New name` renames one.
- *Show this version* (as words) makes the one being edited the track's shown version.

These are done at once, as the versions bar above the editor does them, and they wait until
the editor's changes are applied or discarded: those are for the version they were typed
for.
