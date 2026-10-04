# Review

Press `6`. Review is the workbench for fixing the library.

**Left:** every track with an open *review flag*, each saying why. **Right:** the selected
track's editor, with the cursor already in the first field that needs fixing, plus the
file's name, folder and tags to fix it from.

A row of chips on top filters by kind, with counts; text filters too (the same
[query language](searching.md)). The first chip, **Didn't import**, shows files the server
refused or failed on, which you can fix as drafts and import.

## Where flags come from

- **Ingest and merges:** *fix later*, *possible duplicate*, *ambiguous artist*, *artist
  merged*.
- **Checks the server runs on every track:** no title, no artists, no album, no cover, no
  release date. A flag is raised when a track starts missing one, and settled once it's
  there again. `acetate review-checks` lists them and turns them on or off.
- **Yours:** `flag:note` in the [editor](editing/editor.md).

## Keys

| Key | |
|---|---|
| `Ctrl+Enter` | apply; if nothing is left to fix, mark done and go to the next |
| `Ctrl+↓` / `Ctrl+↑` | next / previous track (even in a field) |
| `j k` / `↓ ↑` | next / previous (outside a field) |
| `Space` | play the track |
| `u` | skip it |
| `d` | mark its flags done (the chosen chip's kind, or all) |
| `Ctrl+M` | compare with its possible duplicate, to [merge](editing/merging.md) |
| `c` on the artist | ambiguous artist: credit the next candidate |
| `Ctrl+V` | paste an image: a new cover |
| `Ctrl+V` | in the input field, paste YouTube link or SoundCloud metadata |
| `b` | bulk tools for what's shown |

Marking many works as in any track list (`x`, `Shift`, `Ctrl+A`); the editor then changes
all of them.

## Bulk tools

With a chip chosen, `b` (or "Bulk…") acts on exactly what's shown. It lists everything
first; nothing happens before the button.

- Any kind: mark the shown flags done, with one note.
- Ambiguous artist: credit each name to one of its candidates, on all its tracks at once.
- No album: give each track a single of its own.
- Possible duplicate: merge the pairs with the same title and about the same length.
