# The track editor

One editor for tracks, wherever you meet it: a track's page in edit mode, the
[Review](../review.md) page, and `Ctrl+E` on a track list (the selected track, or all the
marked ones). It works the way search does: you **type what to change**, `Enter` turns it
into staged changes, you look at the preview, and `Ctrl+Enter` applies.

## The line

```
artist:Ann role:featured
album:Some EP #1
status:demo date:2020
-genre:rap
```

- Several changes fit on one line. If any part can't be understood (no artist by that name),
  *none* of it is staged, and you're told why.
- While you type a part, the choices appear under the line (artists, albums, roles,
  statuses…). `↑`/`↓` choose, `Tab` takes the choice and starts the next part (after an
  artist: `role:`, then `as:`, then its place `#`), `Enter` takes it and stages the line.
- `-genre:rap` takes one value off; `-artist` clears all of them.
- Plain words find things to change by name, or tools: `merge`, `lyrics`, `extra`,
  `history`, `credited as`…
- `Backspace` on an empty line takes the last change back to edit it. `/` returns to the
  line.

## The fields

With one track selected, `↓` from the empty line goes into its fields. `Enter` edits one
(through the line, ready to change), `Delete` takes it off. On an artist: `r` changes the
role, `a` the name it's credited under, `Alt+↑`/`Alt+↓` its place. On a cover: `m` makes it
the main one, `Alt+↑`/`Alt+↓` moves it. On a source: `Enter` corrects it, and `o` says the
file came from it.

With many tracks, each field shows how many tracks differ.

## Covers and sources

These are changes like the rest, applied together with them:

- `cover:paste` (or `Ctrl+V` with an image, or drop an image file), `cover:file`,
  `cover:<album or track>` (a copy of its cover), `-cover:#2`
- `source:<link>`, `-source:#1`, `origin:#2`
- `source:custom vk, sent by a friend`: a source with no platform. The label goes before
  the comma; anything after it is the description.
- `source:distrokid album_uuid:0b358691-d0db-4fa4-b494-1cdf92f034dc index:1`: a platform
  and its fields, typed one by one. Use this when a link doesn't say enough. It works for
  every platform, and fields chain like `artist:` → `role:` → `as:`. `Tab` on a platform
  adds its first field (`source:distr` → `source:distrokid album_uuid:`). After a value,
  `Tab` adds the next field not given yet, needed ones first. The choices under the line
  say what each field takes. A value runs until the next field, and a list field is given
  once per item (`song_permalinks:a/b song_permalinks:a/c`).
- Fields can follow a link or a custom label too: `source:<link> description:sent by a
  friend`, `source:custom vk url:https://…`.
- `name:` and `status:` are editor keys (the title, the status), so Traktrain's fields of
  those names can't be typed after `source:`: they'd change the track's title or status.
- `source:=…` makes it the track's only source, and the file comes from it. This is generally
  the preferred method for fixing generic sources.

## Singles and groupings

`single:` gives each track a single of its own. `group:` puts it in a grouping (a new one
with `kind:`); `-group:` takes it out.

## Importing metadata

Paste (`Ctrl+V`, on an empty line) a SoundCloud track's JSON, a yt-dlp JSON, or a YouTube
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
`Esc` clears the line; again, it discards the changes (asking first).
