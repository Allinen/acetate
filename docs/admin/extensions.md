# Extensions (design draft)

> Status: all of it works: shapes, links and ids (data), pasted-JSON detection (`[import]`) and the
> fetch program (`[fetch]`). YouTube's and SoundCloud's programs are `extensions/*/metadata.py`.

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

# ---- Ids: how this platform's ids look, and where they turn up ----
# Any 11 of A-Z a-z 0-9 - _ (so "helloworld1" is one, on purpose).
[id]
kind = "track"
field = "song_id"
class = "id"            # id (letters, digits, - and _) or digits
len = 11
filename = true         # an id at the end of a yt-dlp file name: `Title [id].ext`
bare = true             # an id pasted on its own is a link to it

# ---- Links: one template does both ways ----
# Pasting a link finds the source; `open` filled in is the source's page.
# `{name}` is a part of the link to keep; scheme, www. and m. don't matter; a last `*` is
# anything more; a query parameter or a #fragment in the template has to be in the link.
[[link]]
kind = "track"
open = "https://www.youtube.com/watch?v={song_id}"
match = [
  "https://youtube.com/watch?v={song_id}",
  "https://music.youtube.com/watch?v={song_id}",
  "https://youtu.be/{song_id}",
  "https://youtube.com/shorts/{song_id}",
]
captures = { song_id = { class = "id", len = 11 } }   # what a part has to look like (also: not = [...])
# comment = true      # a link in a file's comment counts too (the first one that is a source)
# set = { album_permalinks = "{user}/sets/{set}" }    # when the fields aren't the parts as they are

[[link]]
kind = "artist"
open = "https://www.youtube.com/channel/{channel}"
match = ["https://youtube.com/@{channel}", "https://youtube.com/channel/{channel}"]

# ---- Pasted JSON: which one is this platform's ----
# Any of these. The program's `parse` then reads it.
[import]
extractor = "youtube"                     # yt-dlp's extractor_key, any case
keys = []                                 # or it has any of these keys
contains = { webpage_url = "youtube.com/" }   # or the text at a key has this in it

# ---- The part that needs code ----
[fetch]
command = ["python3", "metadata.py"]      # run in the extension's folder
timeout = 60                              # seconds
at_once = 3                               # running together, for everyone
per_user_per_minute = 10
network = true                            # said, so the admin sees it (not enforced: see "Limits")
env = ["ACETATE_YTDLP_COOKIES"]           # the only environment variables it is given (and PATH, HOME, LANG)
thumbnail_hosts = ["ytimg.com"]           # where the server may fetch a thumbnail from
```

A store with no shape of its own (Qobuz, Tidal...) is the same file with no `[[shape]]`:
a `[[link]]` with `kind = "any"`, `custom = "qobuz"` and `hosts = ["qobuz.com"]` makes its links
`custom` sources labelled "qobuz" (`amazon.*` is any ending; `path_contains` narrows it).
See `extensions/stores/extension.toml`.
A platform like NetEase is a manifest and nothing else, and so are the private ones
(a beat-selling site, a distributor, a private archive): they live in the admin's own folder
(`ACETATE_EXTENSIONS_DIR`), not in the repository.

## What is code: the fetch program

The server runs `command` with the extension's folder as the working directory (for the
extensions that come with Acetate, `/app/extensions/<name>` in the image, or
`ACETATE_BUILTIN_EXTENSIONS_DIR`), writes one JSON object to its standard input, and reads one
from its standard output. It gets `PATH`, `HOME`, `LANG`, the variables its manifest lists, and
`ACETATE_FETCH_TIMEOUT` (the seconds it has).

```
in:  { "op": "metadata", "input": "<the link or id as typed>", "match": { "song_id": "dQw4w9WgXcQ" } }
     { "op": "parse", "input": { …the pasted JSON… } }
out: { "from": "YouTube", "title": "…", "date": "2021-06-04", "genre": null,
       "artists": ["…"],
       "account": { "platform": "youtube", "ids": ["UC…"], "label": "youtube @x", "data": { "channel": "UC…" } },
       "source": { "platform": "youtube", "id": "dQw4w9WgXcQ", "data": { …fields of the track shape… } },
       "artwork": ["https://i.ytimg.com/…"] }
```

`match` is what the extension's `[[link]]`s and `[id]` made of the input, so the program
doesn't read links. The answer is the track's metadata as the track editor takes it
(`account` is the uploader's, when the artist is the uploader). Or `{ "error": "why" }`, shown
to the user as it is (with `"unavailable": true` when a tool it needs, like yt-dlp, isn't
installed: the user is told so). Any language works.

The server keeps around it: the timeout, the cap on processes running together, the per-user
limit, answering the same `metadata` request again for 10 minutes without running it, and
fetching the thumbnail itself (only from `thumbnail_hosts`).

Clients ask through `POST /api/extensions/import` (a pasted JSON; the extension whose `[import]`
recognizes it reads it) and `POST /api/extensions/{name}/metadata` (`{"input": "<link>"}`).
YouTube's program is `extensions/youtube/metadata.py` (yt-dlp), SoundCloud's reads a track's
JSON in `extensions/soundcloud/metadata.py`; each has tests next to it
(`python3 -m unittest`).

## Who gets what

* The server loads the built-in extensions (compiled into the image: YouTube, SoundCloud,
  NetEase, QQ, Kuwo) and then the folder `ACETATE_EXTENSIONS_DIR`. A folder
  with a built-in's name replaces it.
* `GET /api/extensions` serves every extension's data (not its program).
  Desktop and phone keep it with the catalog (`clients/core`), so **link parsing is
  written once**, in Rust, in `clients/core`, from the data.
* The Python client reads the same data instead of the generated `_shapes.py`.
* Only the admin installs one: files on the server's disk. Never uploaded through the API.

## Python ingestion scripts

The Python library (`clients/python`) has typed classes for the built-in platforms only. For a
platform of your own, use a `RawSource` once the library knows its shape:

```python
acetate = Acetate(...)            # logged in
acetate.load_extensions()         # asks the server for its extensions, yours included
Description(..., origin=RawSource("traktrain", "track", {"track_id": 123}))
```

Without a server (a dry run, a test), `load_extension_files("path/to/your/extensions")` reads
the same `extension.toml` files (Python 3.11 or newer). Either way the data is checked in the
script before anything is sent, and again by the server.

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
3. The fetch program: YouTube's yt-dlp is `metadata.py`; the server runs programs (done).
4. Author-specific leftovers: the `Singles`/`Albums` folder rule, the cookies setting, the
   hard-coded `"released"`, the vocabulary (its own task).
