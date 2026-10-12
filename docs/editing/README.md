# Editing

Press `e` to turn **edit mode** on or off. While it's off, nothing on screen can change
your library by accident. While it's on, pages grow their editing tools: rows for albums,
forms for artists, the track editor, lyrics, notes, extras and so on. (`Ctrl+E` on a track opens
the editor in either mode, for a quick fix.)

## What you can edit

- **Tracks:** title, artists and their roles, album and position, date, genres, status
  (`released`, `demo`, `unreleased`, `deleted`…), covers, sources, notes, lyrics, related
  tracks. See [the track editor](editor.md); the [track page](track-page.md)'s commandline
  adds notes, extras, original files, earlier files, versions, undoing merges and restoring.
- **Albums:** the same fields as a track as rows you change with the commandline, plus kind (single, EP, mixtape, or your own words) and
  track order (`Alt+↑`/`Alt+↓`, then `Ctrl+Enter`). All of it, plus covers, groupings,
  sources, extras and notes, can be typed on [the album page's commandline](album-page.md).
- **Artists:** name, disambiguation, **aliases** (one artist, several names, tracks still
  sourced in place). Artists have IDs, so two artists may share a name. All of it, plus
  sources, extras and notes, can be typed on [the artist page's commandline](artist-page.md).
- **Groupings:** a collective, label, era or project. Members are added by hand; being
  credited on a song isn't enough. Browse them from Artists. One made by mistake can be
  deleted from its edit panel once it's empty (no tracks, releases, members, sources or
  extras); making one again with the same name brings it back, without its history.
- **Extras:** vocals, stems, samples, music videos: files linked to a track, album, artist
  or grouping, with their own page. A file an extra has already (the same bytes) isn't added
  again, by any route (an upload, an import, turning a track into an extra): the server
  says which extra has it. (Duplicates from before that are merged, below.)
  An extra that is a YouTube video (its name ends with the video's id, `Title [id].mp4`) can
  become a track: in edit mode, *Make it a track from its YouTube video…* on its page (type
  the video's link if the name has no id), then *Look*. The server asks YouTube (with yt-dlp,
  as the editor's "import metadata" does, once) for the title, artist (or the channel), date
  and thumbnail, and the track editor opens on that draft: change what you like (the line
  starts with `parse:prefix feat prod`, to move "Artist -", feat. and prod. out of YouTube's
  title: Enter, look at the check), then apply. The video is the track's source. For a video, the track plays its sound, copied out of the video as it is (needs ffmpeg on the server; a sound that is not AAC, MP3, Opus, Vorbis or FLAC is refused), so playing never depends on the video's container; the video stays an extra, linked to the track, and undoing the track unlinks it. For an extra that is plain audio the track plays the extra's file, and the extra is taken off (kept; *Taken off*
  brings it back). [Review](../review.md)'s "Unlinked extras" does it for many at once.
  Extras that are really one thing (the same file twice, or the pieces of one set) are
  merged on the page you keep: in edit mode, *Merge other extras into this one…*, pick them
  by name, preview what moves, then merge. Their files and links come over (a file or link
  the kept one has already stays behind), and each old extra goes to the trash. They must
  be seen by the same people, so a merge never opens a file to more of them, and an extra
  made from a track has to be made a track again first. Taking an old extra out of the
  trash undoes the merge; an admin can purge it after 7 days (it stays as an empty entry
  with its history).
- **Original files:** mark tracks whose file is a real original ("OG") file, with the name
  it was uploaded as. Needs `edit_files`; you have to *know* it's an original.

## Many at once

Mark tracks with `x` (or `Ctrl+click`, `Shift+click` for a range, `Ctrl+A` for all shown),
then `Ctrl+E` edits all marked at once (in listen mode too). The editor shows how many tracks differ in each
field, and previews every change before you apply it.

In edit mode, other bulk tools act on the marked tracks: *Credited as…* (credit an artist under another of
their names, or fix a name that was really someone else's, on tracks and albums together),
*Mark as original files…*, and [merging](merging.md). Each previews exactly what will change,
and nothing happens until you press the button. `Ctrl+B` opens *Credited as…* (for the marked tracks, or
the selected one); its commandline takes `artist:`, `only:` (`all`, `own` or one of its other
names), `to:` (another artist), `as:` (`own` or an other name) and `why:`, `Enter` sets what's
typed and `Ctrl+Enter` credits the tracks so.

## Saving

Forms save with `Ctrl+Enter` (one revision, with an optional reason) and discard with `Esc`
(it asks first). If someone saved the same thing meanwhile, your changes are kept on top of
theirs instead of overwriting them. Everything you save is in the [history](history.md).

## Parts

- [The track editor](editor.md)
- [The artist page](artist-page.md)
- [The album page](album-page.md)
- [Moving credits out of titles](title-parsing.md)
- [Merging](merging.md)
- [History and undo](history.md)
