# The album page

An album's page has a **commandline** under its header, like the [artist page](artist-page.md).
It speaks the same language as the [track editor](editor.md) and [search](../searching.md):
`key:value`, with `|` between commands. It has the cursor when the page opens, and `/` gets
back to it. It works in listen mode too, for listening, filtering and moving around; in edit
mode it changes the album as well. `Alt+E` (or the word `edit`) turns edit mode on or off
without leaving it.

While you type, the choices appear under the commandline. `↑`/`↓` choose, `Tab` takes the choice
and leaves a space, after which the list shows what can follow (a command's arguments, in any
order), and `Enter` takes it and does the command. Plain words find things by name: `next` finds
*Play next*, `ann` finds the album's artist Ann (to go to their page), `link` finds *Copy link*.
If you're not sure what's there, type a word.

## Listening

```
play:3
queue:next | star
rating:4.5
```

- `play:` plays the album. `play:3` plays it from its third track; `play:` followed by a few
  words of a title works too (the list shows its tracks).
- `queue:` puts it at the end of the queue, `queue:next` before what's queued.
- `star:` stars it, or unstars it if it's starred (`star:yes`, `star:no` and `-star` say
  which). `rating:3.5` is your rating, in half stars; `-rating` clears it.
- *Copy link* and *Make a share link…* (as words) copy its link or open the share popup.

Every key, what it takes and what can follow it, generated from the app: [Commands](../commands.md).

## Filtering and moving around

```
tracks:glass
tracks:year:2019 | og:yes
```

- `tracks:words` filters its tracks, the same way as the library's filter (`year:`, `status:`
  and the rest work in it). `tracks:` alone shows them all again. The list follows while you
  type, and `Enter` keeps it; `Esc` puts back what was there.
- `og:yes` / `og:no` turns the *Original files* chip on or off.
- `credits`, `covers`, `sources`, `notes`, `extras` and `history` open that part of the page
  and scroll to it. A folded part that's already open is closed instead, and the list says
  which it will do. In edit mode, `merges` goes to the merges that can be undone.
- `edit:3` opens the [track editor](editor.md) for its third track (as `Ctrl+E` does, in
  listen mode too). In edit mode, `edit:all` or `edit:2 5` marks those tracks and opens
  the editor for all of them.

## Changing the album (edit mode)

Changes are staged as chips under the commandline. `Ctrl+Enter` saves them all, and `Backspace`
on an empty commandline takes the last one back to edit. Outside edit mode they aren't
suggested, and if you type one anyway it tells you why it can't be done.

- `title:`, `kind:ep`, `date:2019-05-12` (written as the source wrote it; `-date` clears
  it), and `why:`. The reason is kept in the album's [history](history.md) with a change to
  its title, kind, date or artists. On its own there's nothing to save, and saving says so.
- `artist:Name` credits an artist: as primary if the album has none, otherwise as featured.
  Its arguments are `role:producer`, `as:` (one of the artist's other names) and `n:2` (its
  place among the album's artists), in any order. `feat:Name` credits one as featured. An
  artist can be credited in several roles (primary and producer, say): `artist:Ann
  role:producer` adds that credit when Ann has another role. For a credit that's there, the same
  command with its role changes its name or place (`artist:Ann role:primary n:1`); without a role
  it means the artist's only credit. To change a role, take that one off and add the other: `r`
  on its row fills in `-artist:Ann role:featured | artist:Ann n:2 role:`. Under `artist:`
  the list shows the artists that match, then *New artist*, which is made when you save.
  `-artist:Name` takes an artist off (`-artist:Name role:producer`: that role only), and
  `-artist` alone takes them all off.
- `cover:paste` adds the image you copied (an image, or an image file copied in a file
  manager). `cover:choose` opens the file dialog and puts the path in the commandline;
  `cover:/path/to/image.jpg` adds that file (`Tab` completes paths). `cover:` followed by an
  album or track adds a copy of its cover.
- `cover:3` makes the third cover the main one, `cover:3 n:2` moves it to the second
  place, and `-cover:2` puts the second one in the trash. The list numbers them. The words
  *Main cover on all its tracks* put the main cover on each of its tracks that hasn't got it.
- `group:The Crew` puts the album in a grouping. A grouping that doesn't exist yet needs
  its kind: `group:Night kind:era`. `-group:Name` takes it out.
- `move:5 n:2` moves its fifth track (a title works too) to the second place. This goes straight onto the
  tracklist's new order (the same as `Alt+↑`/`Alt+↓`): the bar above the tracks shows it, and
  `Ctrl+Enter` saves it.
- `source:`, `-source:N`, `extra:`, `-extra:`, `note:` and `-note:` work as on the
  [artist page](artist-page.md).

Under the commandline, in edit mode, the album's title, kind, date and artists are listed as
rows, as the [track editor](editor.md) lists a track's: each as it will be once your staged
changes are saved, with what it was crossed out beside it. `↓` on an empty commandline goes
into the rows (`↑`/`↓`, or `j`/`k`, move; `Esc` goes back to the commandline). `Enter` edits
a row: it puts the command for it in the commandline, ready to change. On an artist, `r` is
for its role, `a` for the name it's credited under, `Alt+↑`/`Alt+↓` move it, and `Delete`
takes it off; on the date, `Delete` clears it. These are the same commands you can type,
staged as chips like the rest, and saved with `Ctrl+Enter`.

The album's own changes (title, kind, date, artists) are saved as one revision. If it
fails (someone else changed the album in between, say), what's left unsaved stays in the
rows and a bar says why, with *Load theirs, keep mine on top* when that is the reason; `Ctrl+Enter`
tries again and `Esc` twice discards it. Covers, groupings, sources, extras and notes are saved one after another, in the order
you staged them. If one of them fails, it stays staged and says why, and the rest are still
saved.
