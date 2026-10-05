# Moving credits out of titles

Titles often carry credits: `Song (feat. Ann)`, `Song [prod. Dee]`, `Ann x Bo - Song`.
Acetate can move them into real credits for many tracks at once. It doesn't guess: you
choose **rules** that match how the selected tracks are written, check what every title
turns into, fix the few that came out wrong, and apply.

Why no guessing? `Cool song! [bong]` and `Another song [bang]` look the same, but the
first is just a title and the second means "produced by bang". Only you know which.

## The short version

1. Select the tracks (for example, Review → "Credits in the title", filtered to one artist).
2. Open the editor (Ctrl+E) and type `parse:` with the rules that fit, then Enter:
   ```
   parse:feat prod
   ```
3. Look at the check (↓ from the empty line). Fix what's wrong.
4. Ctrl+Enter applies.

Then do the next artist with whatever rules fit them.

## Rules

| Rule | Finds | Credits as |
|---|---|---|
| `feat` | `feat` `feat.` `ft` `ft.` `featuring` | featured |
| `prod` | `prod` `prod.` `prod by` `produced by` | producer |
| `w/` | `w/` (not `w/o`) | featured |
| `with` | `with` as a word | featured |
| `w` | `w` as a word | featured |
| `x` | `x` as a word, after the title | featured |
| `+feat` / `+prod` | `Song + Ann` | featured / producer |
| `[prod]` / `[feat]` | a bracket with only names in it: `Song [Dee]` | producer / featured |
| `(prod)` / `(feat)` | the same with round brackets | producer / featured |
| `prefix` | `Ann x Bo - Song` ("Artist - Title") | primary |
| `prefix loose` | the same, and a dash with a space on one side only: `Ann x Bo- Song`, `Ann -Song` (never `Jay-Ann`) | primary |
| `self` | `prod. himself`, `prod. me`, `(self-prod.)` | the track's own primary artists |

Type as many as fit: `parse:prefix feat prod`. They always run in the same order (the
keywords first, then `self`, `+`, bare brackets, `prefix`), whatever order you type them in.

`w`, `x`, `with` and the bare brackets can misfire on titles like `With You` or
`Song [Live]`. Only use them on tracks that really are written that way.

### Where a credit ends

- **In brackets**: `Song (feat. Ann, Bo)` is read up to the closing bracket, and the
  brackets go too.
- **Not in brackets**: `Song feat. Ann & Bo` is read up to the first opening bracket,
  another rule's keyword, ` - `, ` | `, or the end.
- **Version words stay**: a bracket with `remix`, `instrumental`, `live`, `official`,
  `v2`, a year and so on in it is never read as names. `Song [Bo Remix]` stays as it is.
- A keyword at the very start of a title isn't a credit: `With You` stays.

### Names

Names are split on `,` `&` `x` `+` `and`: `feat. Ann, Bo & Cy` gives Ann, Bo and Cy.
Signs (`,` `&` `+`) split wherever they are, so `Ann&Bo` is two names too. Words (`x`,
`and`) need a space on each side, so `Max` and `Alexandra` aren't split. An artist name
with a separator in it ("Ann & The Bos", "Ann&Bo") stays in one piece if your library
already has an artist (or alias) called exactly that. If you still need it to split, 
you can press `s` on the artist to split it manually. 

Each name is matched to your library **exactly**, ignoring case:

- **One artist has that name** (or alias): that artist.
- **Nobody has it**: a new artist is made.
- **Several artists have it**: you choose which one in the check before you can apply.

## Changing how it reads

Add these after the rules:

| Change | Example | What it does |
|---|---|---|
| A rule's role | `w/=prod` | `w/ Dee` credits Dee as producer |
| A role for each name | `prefix=primary,featured` | `Ann x Bo x Cy - Song`: Ann primary, Bo and Cy featured (the last role covers the rest) |
| Separators | `split:-x -and` | don't split on ` x ` or ` and ` (for duos like "Ann x Bo") |
| | `split:, &` | split *only* on these |
| | `split:/` | split *only* on `/` (for titles written `feat. Ann / Bo`) |
| | `split:-&` | don't split on `&` (for names like "Ann&Bo" that aren't in your library yet) |
| Keeping names whole | `whole:off` | always split, even names your library has |
| Version words | `keep:tape -live` | add a word that's never a name, or take one off the list |
| Your own keyword | `word:vs=featured`, `word:"beat by"=producer` | works like `feat` |

```
parse:prefix=primary,featured feat prod split:-x
```

## The check

With `parse:` staged (one chip or several), the editor lists every **name** found and
what it credits, your **corrections**, and every **track** that changes: its title now →
as the whole edit leaves it (suffixes taken off and put back included), and the credits
it gains. Whatever needs a look comes first: names to choose, names that look wrong, new
artists, tracks that can't be changed. With several `parse:` chips, each name says which
one found it ("parse 2"), and a correction goes to that chip.

Get there with ↓ from the empty line (with one track, ↓ past the last field).

| Key | On | Does |
|---|---|---|
| Enter | a name | say who it is: the line searches artists (or makes a new one) |
| `c` | a name | the next artist with that name (when several have it) |
| `s` | a name | split it, on every separator, even one turned off with `split:`, or a name your library has (says so if there's nothing to split) |
| `w` | a name | keep a name whole: the line has the text it was found in, so trim it to exactly the name and press Enter |
| `n` | a name | not a name: that part stays in the title (`Cool song! [bong]` keeps `[bong]`) |
| `x` | a track | leave it out (again: back in) |
| Delete | a correction | take it back |

Use the rules and separators for patterns across all the tracks. Use the check for the
few exceptions.

## Suffixes

Some titles end with something that isn't the song, like `Song w/ Ann | Some Tape` or
`Song w/ Ann (BUY 1 GET 1 FREE)`. Take it off, parse, and (if you want) put it back:

```
-suffix:\| | parse:w/ | suffix:
```

- `-suffix:X` takes off everything from the **last** `X` in the title to the end, whatever
  it is: `-suffix:\|`, `-suffix:(`, `-suffix:"| Some Tape"` (any case). A `|` alone
  separates changes, so the one to cut at is escaped (`\|`) or quoted (`"|"`). Titles without it
  are left as they are. Nothing clever: `-suffix:(` on `Song (Remix) [Live]` leaves `Song`, 
  so be careful using it.
- `suffix:` with nothing after it puts back exactly what `-suffix:` took off, each track
  its own.
- `suffix:"| Some Tape"` adds it (after a space), to titles that don't end with it already.

Changes apply in the order you staged them, so you decide when the suffix comes off and
when it goes back on.

## What stays safe

- Nothing is written until you apply, and every track shows exactly what changes.
- Credits are only added. A track's existing credits stay, and an artist already credited
  in that role isn't added twice.
- A track whose title changed after you staged `parse:` is left out, never guessed. Stage
  `parse:` again to include it.
- Tracks that still have `feat.` or `prod.` in their title after applying stay flagged in
  Review.
