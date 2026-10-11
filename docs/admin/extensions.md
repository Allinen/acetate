# Extensions (design draft)

> Status: the data part (shapes, links, ids) works, in `extensions/*/extension.toml`. Pasted-JSON
> import (`[import]`) and the fetch program (`[fetch]`) are designed here and not built yet.

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
a `[[link]]` with `kind = "any"`, `custom = "qobuz"` and `hosts = ["qobuz.com"]` makes its links
`custom` sources labelled "qobuz" (`amazon.*` is any ending; `path_contains` narrows it).
See `extensions/stores/extension.toml`.
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
* `GET /api/extensions` serves every extension's data (not its program).
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
