# Sharing

There are two kinds of link.

- **`acetate://` link** (`Ctrl+L`): a link to the track, album or artist you're looking at,
  for someone with an account. It's copied at once, with no options. It opens straight in
  their desktop app. The app registers the `acetate://` address with the system on Linux and
  Windows.
- **Share link** (`Ctrl+Shift+L`, or right-click → Share with `Shift` held): a web link to a
  track or album that *anyone* can open, no account needed. Creating one needs the `share`
  permission. It opens a popup first.

Right-click a track or album for the Share menu.

## The share popup

It has its own commandline, and the same things by mouse:

| Command | |
|---|---|
| `original:yes` / `original:no` | play the original file, or (the default) a lower-quality copy. Needs `download_original` |
| `quality:64` | the copy's bitrate in kbps, one of the server's (`default` for its usual) |
| `expires:7d` | `never` (the default), `1d`, `7d`, `30d`, or any number of days (`10d`) or weeks (`2w`) |

`original:yes expires:7d` sets both. `Enter` applies what's typed; on an empty line it makes
the link and copies it. `Ctrl+Enter` does both, `Esc` closes.

## What a link plays

By default a share link plays a lower-quality copy (Opus, 128 kbps) with none of the file's
tags or cover, so the link can't be used to collect original files. With **original** the link
plays the original file. If its creator later loses `download_original`, the link goes back
to playing copies. The desktop app's Settings → Account lists the links, says which kind each is and revokes them (`GET /api/shares`, `DELETE /api/shares/{id}` for scripts).
