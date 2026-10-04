# Editing

Press `e` to turn **edit mode** on or off. While it's off, nothing on screen can change
your library by accident. While it's on, pages grow their editing tools: forms for albums
and artists, the track editor, lyrics, notes, extras and so on. (`Ctrl+E` on a track opens
the editor in either mode, for a quick fix.)

## What you can edit

- **Tracks:** title, artists and their roles, album and position, date, genres, status
  (`released`, `demo`, `unreleased`, `deleted`…), covers, sources, notes, lyrics, related
  tracks. See [the track editor](editor.md).
- **Albums:** the same kind of form, plus kind (single, EP, mixtape, or your own words) and
  track order (`Alt+↑`/`Alt+↓`, then `Ctrl+Enter`).
- **Artists:** name, disambiguation, **aliases** (one artist, several names, tracks still
  sourced in place). Artists have IDs, so two artists may share a name.
- **Groupings:** a collective, label, era or project. Members are added by hand; being
  credited on a song isn't enough. Browse them from Artists.
- **Extras:** vocals, stems, samples, music videos: files linked to a track, album, artist
  or grouping, with their own page.
- **Original files:** mark tracks whose file is a real original ("OG") file, with the name
  it was uploaded as. Needs `edit_files`; you have to *know* it's an original.

## Many at once

Mark tracks with `x` (or `Ctrl+click`, `Shift+click` for a range, `Ctrl+A` for all shown),
then `Ctrl+E` edits all marked at once. The editor shows how many tracks differ in each
field, and previews every change before you apply it.

Other bulk tools act on the marked tracks: *Credited as…* (credit an artist under another of
their names, or fix a name that was really someone else's, on tracks and albums together),
*Mark as original files…*, and [merging](merging.md). Each previews exactly what will change,
and nothing happens until you press the button.

## Saving

Forms save with `Ctrl+Enter` (one revision, with an optional reason) and discard with `Esc`
(it asks first). If someone saved the same thing meanwhile, your changes are kept on top of
theirs instead of overwriting them. Everything you save is in the [history](history.md).

## Parts

- [The track editor](editor.md)
- [Moving credits out of titles](title-parsing.md)
- [Merging](merging.md)
- [History and undo](history.md)
