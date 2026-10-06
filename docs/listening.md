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
Settings is `Ctrl+,`.

## The command palette

`Ctrl+K`, from anywhere. Type to find any artist, album, track, playlist or command: letters
in order, words anywhere. `Enter` opens it (a track: plays it, then the rest of its album),
`Shift+Enter` plays it (a track: opens its album). Start with `>` to list only commands.

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

## Playlists

Make one from the `+` in the sidebar, or with `p` on anything. Inside a playlist:
`Delete` takes a track out, `Ctrl+↑`/`Ctrl+↓` reorders, `F2` renames. Deleted playlists go
to the [trash](library/storage.md) and can be restored.

## Quality and downloads

With the `download_original` permission, you stream the original files. Without it you stream
a transcoded copy (Opus, 128 kbps by default; other bitrates if the admin lists them: `?kbps=64` on the stream, `GET /api/qualities` says which), made the first time anyone plays the track
(a few seconds, then it's kept). A copy has none of the original's tags or cover. Videos
aren't transcoded.

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
automatically).
