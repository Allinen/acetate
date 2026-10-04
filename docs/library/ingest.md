# Importing music

```sh
acetate ingest <dir> --layout <layout> [options]
```

Reads every audio file under `<dir>`. The metadata comes from the tags; the *layout* says what
the folder names tell you (usually platform ids). Always try `--dry-run` first: it prints what
would happen and changes nothing.

Under Docker, run it inside the container (`docker compose exec acetate acetate ingest …`)
with the path as the container sees it.

## Layouts

| `--layout` | For |
|---|---|
| `tags` | nothing from the path, only tags. Needs `--origin` |
| `netease`, `qq`, `kuwo` | downloads like `Artist - <id>/…/Album - <id>/… - <song id>.flac` |
| `soundcloud` | `<user id>/{FLACs\|streams}/…` |
| `instrumentals` | `<producer>/<site>@<account>/…` |
| `ytdlp` | yt-dlp's `Title [id].ext`; add `--platform soundcloud` or `youtube` |

Layouts never guess: a path that doesn't fit gives no ids and a warning. (To support another
downloader, a layout is a small Rust module in `src/ingest/layouts/`.)

## Useful options

| Option | |
|---|---|
| `--origin <label>` | where the files came from when the path doesn't say (`--origin vk`) |
| `--dry-run` | report only |
| `--status <s>` | initial status of new tracks (default `released`) |
| `--restrict-to <groups>` | create tracks visible only to these groups |
| `--volume <name>` | which [storage volume](storage.md) |
| `--flag "note"` | put every new track on [Review](../review.md) |
| `--include` / `--exclude` | globs, relative to `<dir>` |
| `--resume-from N` | carry on after an interruption |
| `--reflink-only` | stop rather than copy a file that can't be reflinked |
| `--covers <dir>` | album covers named `<title> - <album id>.jpg` (netease) |
| `--no-composers` | don't credit the COMPOSER tag as producers |
| `--no-merge-duplicates` | a file already stored only gets the new origin |
| `--server <url>` | send files to a remote Acetate over HTTP instead of importing locally. `ACETATE_TOKEN` env var is useful to not have to login. |

Run `acetate ingest --help` for the complete list, including id maps and name matching.

A file that is already stored isn't stored again: what the import knows that its track
doesn't (another album, covers, ids) is merged in.

## Other ways in

- **Upload** from the apps (needs the `upload` permission). Uploads are limited per request
  and per day; see [Configuration](../admin/configuration.md).
- **`acetate import --origin <label> <files…>`** imports individual files.
- **`acetate videos <dir>`** imports music videos as extras, to link from their pages.
- **Python library** `acetate_ingest` (`clients/python`) for scripts tailored to your own
  library: describe what you know about each file (artists, album, platform ids, covers,
  lyrics) and send it.

  ```python
  from acetate_ingest import Acetate, Description, ArtistHint

  acetate = Acetate.from_env()
  report = acetate.ingest("Song.flac", Description(artists=[ArtistHint("Ann")]))
  print(report.summary())
  ```

## Checking afterwards

`acetate verify <dir>` confirms (read-only) that every file under `<dir>` is safely stored,
for example before you delete an old library by hand. `acetate verify --blobs` checks the
blob store itself.
