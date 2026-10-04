# Merging

Two tracks, albums or artists that are really one can be merged. The metadata of both stays
as **versions**, so nothing is lost. Track and album merges can be undone.

## Tracks

From a [Review](../review.md) "possible duplicate" flag, from two marked tracks (`Ctrl+M`),
or from a track's page in edit mode (`Ctrl+M`, then choose the other).

The two are shown side by side. **Left is kept**; the right is merged into it. Each row
(title, artists, date, genres, comment, status, file, covers) has a pick for whose it is;
rows where both agree are skipped over. The file row shows format and quality, and you can
play either side (`Space` for the left, `Shift+Space` for the right, continuing from the same
moment, to hear the difference).

Albums, relations, sources and lyrics are combined automatically; the view lists them.
Nothing happens until `Ctrl+Enter`. If either track changed meanwhile, the merge is
refused rather than undone.

| Key | |
|---|---|
| `↑ ↓` / `j k` | row |
| `← →` / `h l` | take the left / right one |
| `s` | take everything from the right; again, from the left (the sides stay) |
| `a` | merge their singles along, or not |
| `Ctrl+Enter` | merge |
| `n` | "not the same" (settles the flag) |
| `Esc` | close |

Undo from the kept track's page ([History](history.md)).

## Albums and artists

On the page, in edit mode: `Ctrl+M`, choose the other, `s` to swap which stays, `Ctrl+Enter`
to merge. Merging albums can also merge their tracklists. Undoing an **artist** merge isn't
possible yet, so be careful!

## In bulk

`acetate merge-duplicates` merges every "possible duplicate" pair with exactly the same
title and about the same length, and keeps the better file (WAV over FLAC over MP3, then
the bigger). Review's bulk tools do the same after showing exactly which pairs and can be
filtered, so it should be preferred.

`acetate duplicate-albums` lists albums that look like the same album twice. 
Run with `--merge` to merge them. Tracks are not merged. 
