# Listening

The desktop app's left sidebar has six sections, each on a number key:

| Key | Section | |
|---|---|---|
| `1` | Albums | |
| `2` | Artists | also leads to collectives and groupings |
| `3` | Tracks | every track in the library |
| `4` | Now playing | lyrics (synced when available), visualizer. Videos not available yet |
| `5` | Queue | |
| `6` | Review | for fixing the library: see [Review](review.md) |

Below the sections are your playlists, with **Liked songs** (your starred tracks) first.
Settings is `Ctrl+,`. It has tabs (Sound, This computer, Account, and for admins Server and Mirror, then General); `h`/`l` or the arrows move between them.

## The command palette

`Ctrl+K`, from anywhere. Type to find any artist, album, track, playlist or command: letters
in order, words anywhere. `Enter` opens it (a track: plays it, then the rest of its album),
`Shift+Enter` plays it (a track: opens its album).

The line is a [commandline](searching.md): `artist:`, `album:`, `year:`, `status:` and the other
search keys filter. `go:` lists the pages (`go:albums`, `go:queue`) and `do:` the other commands
(`do:pause`, `do:shuffle`); each command shows its `go:`/`do:` form under its name. With nothing
typed, the keys are listed too. `Tab` writes the highlighted key or command in full (on anything
else it moves down), `Enter` on a key writes it and lets you keep typing. `Esc` empties the line,
and closes the palette when it's already empty.

## Playing

- `Space` play/pause, `Ctrl+←`/`Ctrl+→` previous/next, `Shift+←`/`Shift+→` seek 10 s.
- `r` loops the current track, `z` shuffles, `m` mutes, `9`/`0` volume.
- `Shift+S` stars what's playing. In a list, `s` stars the selected track and `+`/`−` change
  its rating by half a star.
- `q` queues the selected track at the end; `Shift+Q` queues it first.
- `p` adds to a playlist (typing a new name offers to create it).
- Playback is gapless. Plays are counted.

## Lists

All lists work the same way: arrows or `h j k l` to move, `Enter` to open or play from
here, `i` for a track's page, and `/` to [filter](searching.md) the list.

In a track list, mark several tracks to do something to all of them: `x` or `Ctrl+click`
marks one, `Shift+click` (or `Shift+↑`/`↓`) marks everything from the track the cursor is on,
`Ctrl+A` marks all shown, `Esc` clears. While tracks are marked, `s` stars them all (or
unstars them, if all were starred), `+`/`−` rate them, `p` adds them to a playlist, and `q`
queues them (`Shift+Q`: first). With the `edit_metadata` permission, `Ctrl+E` edits them
together; [edit mode](editing/README.md) adds merging and the other editing tools.

## Playlists

Make one from the `+` in the sidebar, or with `p` on anything. Inside a playlist:
`Delete` takes a track out, `Ctrl+↑`/`Ctrl+↓` reorders, `F2` renames. Deleted playlists go
to the [trash](library/storage.md) and can be restored.

## Quality and downloads

With the `download_original` permission, you stream the original files. Without it you stream
a transcoded copy (Opus, 128 kbps by default; other bitrates if the admin lists them: `?kbps=64` on the stream, `GET /api/qualities` says which), made the first time anyone plays the track
(a few seconds, then it's kept). A copy has none of the original's tags or cover. Videos
aren't transcoded.

**Streaming quality** (desktop Settings → Sound; the web UI's account page) lets you pick one
of the server's copy qualities (for less to download), or Automatic. It applies from the next
track you play. The queue's next track is fetched again at the new quality.

**Equalizer** (desktop Settings → Sound): ten bands, an octave apart (31 Hz to 16 kHz),
±12 dB each; double-click a band to flatten it. It's heard at once, applies to everything
played, and is remembered. To keep a boost from clipping, the whole sound is lowered by the
largest boost (the settings say by how much).

The web UI's **Download** on a track page (needs `download_original`) gives the original
file with the library's *current* title, artists, album, track and disc number, date, genres,
lyrics and main cover written into its tags. The stored file is never changed: the tags go
into a copy. Add `?tags=false` to the address to get the stored bytes untouched.

## Lyrics and videos

Tracks can have lyrics, synced or plain, shown on Now playing. Music videos are *extras*
linked to a track, and play in the app. Both are added in [edit mode](editing/README.md).

## Working offline

The app keeps the whole library's metadata on your computer, so browsing and searching
don't touch the server. `F5` checks the library for changes (this also happens
automatically). Without the server, what's on your computer still plays, and stars, ratings,
plays and playlist additions wait to be sent: see [Offline](offline.md).
