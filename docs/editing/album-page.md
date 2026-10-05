# The album page

An album's page has a **commandline** under its header, like the [artist page](artist-page.md).
It speaks the same language as the [track editor](editor.md) and [search](../searching.md):
`key:value`, with `|` between commands. It has the cursor when the page opens, and `/` gets
back to it. It works in listen mode too, for listening, filtering and moving around; in edit
mode it changes the album as well. `Alt+E` (or the word `edit`) turns edit mode on or off
without leaving it.

While you type, the choices appear under the commandline. `↑`/`↓` choose, `Tab` takes the choice,
and `Enter` takes it and does the command. Plain words find things by name: `next` finds
*Play next*, `ann` finds the album's artist Ann (to go to her page), `link` finds *Copy link*.
If you're not sure what's there, type a word.

## Listening

```
play:#3
queue:next | star
rating:4.5
```

- `play:` plays the album. `play:#3` plays it from its third track; `play:` followed by a few
  words of a title works too (the list shows its tracks).
- `queue:` puts it at the end of the queue, `queue:next` before what's queued.
- `star:` stars it, or unstars it if it's starred (`star:yes`, `star:no` and `-star` say
  which). `rating:3.5` is your rating, in half stars; `-rating` clears it.
- *Copy link* and *Copy a share link* (as words) copy its link.

## Filtering and moving around

```
tracks:glass
tracks:year:2019 | og:yes
```

- `tracks:words` filters its tracks, the same way as the library's filter (`year:`, `status:`
  and the rest work in it). `tracks:` alone shows them all again. The list follows while you
  type, and `Enter` keeps it; `Esc` puts back what was there.
- `og:yes` / `og:no` turns the *Original files* chip on or off.
- `credits`, `covers`, `sources`, `notes` and `extras` open that part of the page and scroll
  to it. In edit mode, `merges` goes to the merges that can be undone.

## Changing the album (edit mode)

Changes are staged as chips under the commandline. `Ctrl+Enter` saves them all, and `Backspace`
on an empty commandline takes the last one back to edit. Outside edit mode they aren't
suggested, and if you type one anyway it tells you why it can't be done.

- `title:`, `kind:ep`, `date:2019-05-12` (written as the source wrote it; `-date` clears
  it), and `why:` (kept with the change).
- `artist:Name` credits an artist: as primary if the album has none, otherwise as featured.
  Its arguments are `role:producer`, `as:` (one of the artist's other names) and `#2` (its
  place among the album's artists). `feat:Name` credits one as featured. For an artist the
  album credits already, the same command changes its role, name or place. Under `artist:`
  the list shows the artists that match, then *New artist*, which is made when you save.
  `-artist:Name` takes one off, and `-artist` alone takes them all off.
- `cover:paste` adds the image you copied (an image, or an image file copied in a file
  manager). `cover:choose` opens the file dialog and puts the path in the commandline;
  `cover:/path/to/image.jpg` adds that file (`Tab` completes paths). `cover:` followed by an
  album or track adds a copy of its cover.
- `cover:#3` makes the third cover the main one, `cover:#3 #2` moves it to the second
  place, and `-cover:#2` puts the second one in the trash. The list numbers them. The words
  *Main cover on all its tracks* put the main cover on each of its tracks that hasn't got it.
- `group:The Crew` puts the album in a grouping. A grouping that doesn't exist yet needs
  its kind: `group:Night kind:era`. `-group:Name` takes it out.
- `move:#5 #2` moves its fifth track to the second place. This goes straight onto the
  tracklist's new order (the same as `Alt+↑`/`Alt+↓`): the bar above the tracks shows it, and
  `Ctrl+Enter` saves it.
- `source:`, `-source:#N`, `extra:`, `-extra:`, `note:` and `-note:` work as on the
  [artist page](artist-page.md).

The album's own changes (title, kind, date, artists) are saved as one revision, through its
form. Covers, groupings, sources, extras and notes are saved one after another, in the order
you staged them. If one of them fails, it stays staged and says why, and the rest are still
saved.
