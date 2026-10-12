# The track editor

One editor for tracks, wherever you meet it: a track's page in edit mode, the
[Review](../review.md) page, and `Ctrl+E` on a track list (the selected track, or all the
marked ones). It works the way search does: you **type what to change**, `Enter` turns it
into staged changes, you look at the preview, and `Ctrl+Enter` applies.

## The commandline

```
artist:Ann role:featured
album:Some EP n:1
status:demo | date:2020
-genre:rap
```

- Several changes fit on one line, with `|` between them. (The [artist page](artist-page.md)
  and the [album page](album-page.md) have commandlines of their own in the same
  language. On a [track's page](track-page.md) this commandline is the page's: it has the
  page's keys too, and it's there in listen mode.) If any of them can't be understood (no artist by that name), *none* of it is
  staged, and you're told why.
- A change is a key, its value, and that key's own arguments, in any order: `role:`, `as:`
  and `n:` (its place) after an artist, `kind:` and `n:` (its numbers: `n:5`, `n:2-5` for
  disc 2 track 5, `n:1+` in order from 1) after an album, `kind:` after a grouping or a
  related track, a source's fields after `source:`. Anything else is part of the value, so
  `title:Intro: Part 2` needs no quotes.
  A value with a `|` in it is quoted (`title:"guy | more"`) or escaped (`title:guy \| more`).
- While you type a part, the choices appear under the commandline (artists, albums, roles,
  statuses…). `↑`/`↓` choose, `Tab` takes the choice and leaves a space, after which the
  list shows what can follow it (after an artist: `role:`, `as:`, `n:`), and `Enter` takes
  it and stages it. The filter's keys can be typed by their start (`art:`, `gen:`, `stat:`).
- `-genre:rap` takes one value off; `-artist` clears all of them.
- Plain words find things to change by name, or tools: `credited as`, merging two marked
  tracks… On a track's page they find the page's actions too: `merge`, `extra`, `lyrics`,
  `history`…
- `Backspace` on an empty commandline takes the last change back to edit it. `/` returns to the
  line.

## The fields

With one track selected, `↓` from the empty commandline goes into its fields. `Enter` edits one
(through the commandline, ready to change), `Delete` takes it off. On an artist: `r` changes the
role, `a` the name it's credited under, `Alt+↑`/`Alt+↓` its place. On a cover: `m` makes it
the main one, `Alt+↑`/`Alt+↓` moves it. On a source: `Enter` corrects it, and `o` says the
file came from it.

With many tracks, each field shows how many tracks differ.

## Covers and sources

These are changes like the rest, applied together with them:

- `cover:paste` (or `Ctrl+V` with an image, or drop an image file), `cover:file`,
  `cover:<album or track>` (a copy of its cover: `which:2` its second one), `-cover:2`.
  `n:` says where a new one goes (`cover:paste n:1`: the main one), and `cover:3 n:1` makes
  its third cover the main one. A track or album has each image once: adding one it has
  already is refused.
- `source:<link>`, `-source:1` (its first to the trash), `source:1` (correct its first),
  `origin:2` (the file came from its second)
- `source:custom vk, sent by a friend`: a source with no platform. The label goes before
  the comma; anything after it is the description.
- `source:qq song_id:1234 song_mid:0039MnYb0qxYhV`: a platform
  and its fields, typed one by one. Use this when a link doesn't say enough. It works for
  every platform: after the platform and a space, the list shows its fields (needed ones
  first) and what each takes. A value runs until the next field, and a list field is given
  once per item (`song_permalinks:a/b song_permalinks:a/c`).
- Fields can follow a link or a custom label too: `source:<link> description:sent by a
  friend`, `source:custom vk url:https://…`.
- After `source:`, any `name:` is one of its fields, `name:` and `status:` too (a change of
  the track's title or status goes after a `|`).
- `source:=…` makes it the track's only source, and the file comes from it. This is generally
  the preferred method for fixing generic sources.

## Singles and groupings

`single:` gives each track a single of its own. `group:` puts it in a grouping (a new one
with `kind:`); `-group:` takes it out.

## Importing metadata

Paste (`Ctrl+V`, on an empty commandline) a SoundCloud track's JSON, a yt-dlp JSON, or a YouTube
link. The editor stages what it says: title, date, genre, artists, the source, and the
artwork as a cover.

## Review flags

`flag:Some note` flags the track (a name no kind has makes a new kind, listed on Review).
`-flag:…` settles its open flags of that kind.

## Credits in titles

`parse:feat prod` pulls `feat.` and `prod.` credits out of titles. It has its own page:
[Moving credits out of titles](title-parsing.md).

## The preview

Each track's changes are shown in the history's words, plus the albums that change with them
(`along:off` to leave those alone), and its review flags afterwards: fixed, still there, new.
The flags come from the server's own checks. `Ctrl+Enter` applies what you see.
`Esc` clears the commandline; again, it discards the changes (asking first).
