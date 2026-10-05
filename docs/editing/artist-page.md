# The artist page

An artist's page has a **commandline** under its header. It speaks the same language as the
[track editor](editor.md) and [search](../searching.md): `key:value`, with `|` between
commands. It has the cursor when the page opens, and `/` gets back to it. It works in listen
mode too, for moving around and filtering; in edit mode it changes the artist as well.
`Alt+E` (or the word `edit`) turns edit mode on or off without leaving it.

While you type, the choices appear under the commandline. `↑`/`↓` choose, `Tab` takes the choice,
and `Enter` takes it and does the command. Plain words find things by name: `rel` finds
Releases, `merge` finds *Merge into another artist…*. If you're not sure what's there, type
a word.

## Moving around and filtering

```
alias:Zee
og:yes | tracks
tracks:year:2019
```

- `alias:Name` turns that name's chip on or off: the same chips as under the artist's name.
  Several can be on at once. The page follows while you type, and `Enter` keeps it.
  `alias:all` goes back to all names.
- `og:yes` / `og:no` turns the *Original files* chip on or off.
- `releases`, `tracks`, `featured`, `produced`, `extras` show that section on its own.
  `tracks:words` also fills in its filter. `Esc` goes back to the whole page.
- `sources` and `notes` open that part of the page and scroll to it.
- Releases on their own are a grid: arrows or `hjkl` move, `Enter` opens one, `Shift+Enter`
  plays it.

## Changing the artist (edit mode)

Changes are staged as chips under the commandline. `Ctrl+Enter` saves them all, and `Backspace` on
an empty commandline takes the last one back to edit. Outside edit mode they aren't suggested, and
if you type one anyway it tells you why it can't be done.

- `alias:New name` adds another name the artist can be credited as. Under `alias:` the list
  shows the names to filter by first, then *New name*, exactly as typed. A name that
  differs only in case is a different name.
- `-alias:Name` takes a name off. The server is asked first where it's still credited. If
  it's credited anywhere, it isn't staged; the credits are listed as links, so you can
  change them first.
- `name:` renames it. `disambiguation:US rapper` says what tells it apart from other
  artists with the same name (`-disambiguation` clears it). `why:` is kept with the change.
- `source:<link>`, `source:custom vk, their page`, or `source:soundcloud
  artist_permalinks:…` adds a source, the same way as in the track editor. `-source:#2`
  puts the second one in the trash (the edit panel numbers them).
- `extra:Name` makes an extra, changes one of the artist's, or links one from the library.
  Its arguments:
  - `kind:vocals`
  - `file:/path/to/file.wav`: `Tab` completes paths. `file:paste` uses the files you copied
    in a file manager; `file:choose` opens the file dialog. Both put the paths in the commandline,
    so you see what will be uploaded.
  - `description:…`, `source:…` (where it came from)
  - `visibility:everyone`, `visibility:artist`, or `visibility:Group A, Group B`

  `-extra:Name` takes it off this artist. The extra itself stays, along with anything else
  it's linked to.
- `note:Title` opens the note editor: the note with that title, or a new one. `note:paste`
  lists the text files you copied, one note each, and `Ctrl+Enter` makes them. `-note:Title`
  takes a note off (it's kept; *Taken off* brings it back).

The artist's own changes (names, rename, disambiguation) are saved as one revision, through
its form. Sources, extras and notes are saved one by one. If one of them fails, it stays
staged and says why, and the rest are still saved.

## Notes

The note editor opens with the cursor in the text. `Ctrl+Enter` saves the note. `Esc`
leaves the text; `Esc` again asks, and a third time throws the note away.
