# Extensions (design draft)

> Draft: nothing here exists yet. It is written down to be agreed first.

An **extension** teaches Acetate about one place music comes from (YouTube, SoundCloud,
NetEase, a store...). Today that knowledge is spread over the server and every client:
the shapes of a platform's sources, how a pasted link becomes a source, how a link is
found in a file's comment, what a downloader's file name means, how a video's metadata
is read. Five parsers know YouTube; the phone and the desktop each need a rebuild to learn
a new site.

An extension is **a folder** the server reads at start. Most of it is data, so the clients
get the same data from the server and nothing is written twice.

```
extensions/
  youtube/
    extension.toml     what it is, in data
    metadata.py        (optional) the part that needs code or the network
```

## What is data: `extension.toml`

```toml
[extension]
name = "youtube"            # the platform name stored with every source
title = "YouTube"
api = 3                     # the API_VERSION it needs; a server that can't run it says so and skips it

# ---- Sources: what is kept for a track, album or artist on this platform ----
[[shape]]
kind = "track"
id = "song_id"              # the field that says which source it is (snapshots with the same one are one source)
fields = [
  { name = "song_id", type = "str", required = true, pattern = "[A-Za-z0-9_-]{11}" },
  { name = "song_release_date", type = "str" },
]

[[shape]]
kind = "artist"
id = "channel"
fields = [{ name = "channel", type = "str", required = true }]

# ---- Links: one template does both ways ----
# Pasting a link finds the source; the source's page is the template filled in.
# {name} is a field of the shape; a link matches if its host and path fit and the
# {fields} are found where the template has them.
[[link]]
kind = "track"
open = "https://www.youtube.com/watch?v={song_id}"     # shown as the link to its page
match = [
  "https://youtube.com/watch?v={song_id}",             # (www., m. and music. are the same host)
  "https://youtu.be/{song_id}",
  "https://youtube.com/shorts/{song_id}",
]

[[link]]
kind = "artist"
open = "https://www.youtube.com/channel/{channel}"
match = ["https://youtube.com/@{channel}", "https://youtube.com/channel/{channel}"]

# ---- Where a file came from, by its name: yt-dlp's `Title [id].ext` ----
[filename]
bracket_id = { kind = "track", field = "song_id" }

# ---- Where it came from, by a link in its comment ----
[comment]
links = true                # the `match` templates above count in comments too (the first one found)

# ---- Pasted JSON: a downloader's or the site's own, as SoundCloud's is read today ----
# `detect`: the JSON is this platform's if it has any of these keys (a dotted path), or a
# string in `contains` has the text. Then each field of the result is read from a path of
# the JSON; `{a}` in a text is the value at path `a`; a list of paths is the first found.
[import]
detect = { extractor = "youtube", keys = [], contains = { webpage_url = "youtube.com/" } }
title = "title"
artists = ["artist", "creator", "channel"]
release_date = "upload_date"            # YYYYMMDD or ISO, both read
thumbnail = "thumbnail"
source = { song_id = "id" }             # the fields of the track shape
account = { kind = "artist", name = "channel", source = { channel = "channel_id" } }
# what a path can't say (SoundCloud's artwork sizes, a list of descriptions) is left to the
# fetch program: `{"op": "parse", "input": <the JSON>}`, answered like "metadata" below.

# ---- The part that needs code: metadata of a video ----
[fetch]
command = ["python3", "metadata.py"]
timeout = 60                # seconds
at_once = 3                 # processes running together, for everyone
per_user_per_minute = 10
network = true              # said, so the admin sees it (see "Limits" below)
env = ["ACETATE_YTDLP_COOKIES"]   # the only environment variables it is given
```

A store with no shape of its own (Qobuz, Tidal...) is the same file with no `[[shape]]`:
a `[[link]]` with `custom = "qobuz"` makes its links `custom` sources labelled "qobuz".
A platform like NetEase is a manifest and nothing else, and so are the private ones
(Traktrain, BeatStars): they live in the admin's own folder, not in the repository.

## What is code: the fetch program

The server runs `command` with the extension's folder as the working directory, writes
one JSON object to its standard input, and reads one from its standard output:

```
in:  { "op": "metadata", "input": "dQw4w9WgXcQ" }       # a link or an id, as the user gave it
     { "op": "parse", "input": { …pasted JSON… } }      # for what [import] can't read
out: { "id": "dQw4w9WgXcQ",
       "title": "…", "artists": ["…"], "release_date": "2021-06-04",
       "thumbnail": "https://i.ytimg.com/…",
       "source": { …fields of the track shape… },
       "accounts": [{ "kind": "artist", "name": "…", "source": { "channel": "…" } }] }
```

or `{ "error": "why" }` (shown to the user as it is). Any language works. YouTube's does
what `api/metadata.rs` does with yt-dlp now. The server keeps what it does today around it:
the timeout, the cap on processes running together, the per-user limit, reading the same
video once for 10 minutes, and fetching the thumbnail itself (only from hosts the answer's
extension lists in `thumbnail_hosts`).

## Who gets what

* The server loads the built-in extensions (compiled into the image: YouTube, SoundCloud,
  NetEase, QQ, Kuwo, DistroKid) and then the folder `ACETATE_EXTENSIONS_DIR`. A folder
  with a built-in's name replaces it.
* `GET /api/extensions` serves every extension's data (not its program) with an ETag.
  Desktop and phone keep it with the catalog (`clients/core`), so **link parsing is
  written once**, in Rust, in `clients/core`, from the data.
* The Python client reads the same data instead of the generated `_shapes.py`.
* Only the admin installs one: files on the server's disk. Never uploaded through the API.

## Old data

The database keeps the platform name and the data of every snapshot. When an extension is
gone or its shape changed, snapshots already stored stay readable: shown as the stored
fields, read-only, with "no extension for this" beside them. New snapshots are checked
against the shape that is there now. No compatibility code: an extension that needs a newer
`api` than the server has is skipped with a line in the log.

## Limits (what is and isn't promised)

The fetch program runs as the server's user, with only the environment variables its
manifest names, a time limit, and the caps above. **It is not sandboxed**: `network` says
what it does, it doesn't stop it. Install only what you trust, as you would any script. (If
`bwrap` is installed, a later step can run it without network when `network = false`.)

## Order of work

1. This draft, agreed.
2. The data: shapes, links, filenames, comment links and pasted-JSON import as manifests; the built-ins written;
   `/api/extensions`; `clients/core` reads them (one link parser); the generated Python
   shapes go. YouTube's *fetch* still in Rust.
3. The fetch program: YouTube's yt-dlp becomes `metadata.py`; the server runs programs
   through a `MetadataProvider` trait.
4. Author-specific leftovers: the `Singles`/`Albums` folder rule, the cookies setting, the
   hard-coded `"released"`, the vocabulary (its own task).
