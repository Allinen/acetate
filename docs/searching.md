# Searching and filtering

The same query language is used in the `/` filter on any list and in the `Ctrl+K` palette.

| Query | Finds |
|:--|:--|
| `words` | anywhere in title, artists or album |
| `artist:lil quill` | only in that field (no quotes needed) |
| `album:…`, `title:…` | likewise |
| `year:2019` | released that year |
| `genre:…`, `status:demo` | genre, status |
| `format:flac` | file format |
| `flag:cover` | tracks with that kind of [review flag](review.md) |
| `kind:ep` | album kind |
| `disc:2`, `track:5` | disc and track number |

Field names can be shortened to any start: `art:`, `al:`, `st:`.

Searching uses the catalog on your computer, so it is instant and works offline.
