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
| `d` (`Ctrl+D` in a text field too) | mark its flags done (the chosen chip's kind, or all) |
| `Ctrl+M` | compare with its possible duplicate, to [merge](editing/merging.md) |
| `c` on the artist | ambiguous artist: credit the next candidate |
| `Ctrl+V` | paste an image: a new cover |
| `Ctrl+V` | in the input field, paste YouTube link or SoundCloud metadata |
| `b` | bulk tools for what's shown |

Marking many works as in any track list (`x`, `Shift`, `Ctrl+A`); the editor then changes
all of them.

## Unlinked extras

The **Unlinked extras** chip lists extras that belong to nothing yet (videos imported from
disk, for one). They are marked like tracks are: the box in front, `x`, Ctrl+click,
Shift+click or Shift+↑/↓ for a range, `Ctrl+A` for all shown, `Esc` to clear (the text filter
narrows the list first). Then choose what they belong to (track, album, artist or grouping),
type its name and press Enter: every marked extra is linked to it, and leaves the list.

The last choice, **make tracks (YouTube)**, is for videos downloaded from YouTube (their
names end with the video's id, `Title [id].mp4`): **Look** asks YouTube what each marked
extra would become (title, artist, video), and lists the ones that can't (no id, a video
YouTube can't read, a file that is a track already); **Make** turns the rest into tracks and
takes the extras off (kept). Needs yt-dlp on the server.

With nothing marked, the extra under the cursor is the one, and its name is typed into the
search for you: click a row (or move to it), check the match, Enter. Enter on a row puts the
cursor in the search, `Esc` takes it out. `i`, "Open" on a row or a double-click goes to the
extra's own page; unlinking is done there.

## Bulk tools

With a chip chosen, `b` (or "Bulk…") acts on exactly what's shown. It lists everything
first; nothing happens before the button.

- Any kind: mark the shown flags done, with one note.
- Ambiguous artist: credit each name to one of its candidates, on all its tracks at once.
- No album: give each track a single of its own.
- Possible duplicate: merge the pairs with the same title and about the same length.
