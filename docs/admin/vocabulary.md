# Vocabulary

The words Acetate knows are data, kept in one place and changed from the app: **Settings →
Vocabulary** (admins). There are seven lists:

| List | What it holds |
|---|---|
| Statuses | where a track stands: `released`, `demo`, `unreleased`… |
| Credit roles | how an artist is on a track or album: `primary`, `featured`, `producer`… |
| Album kinds | `album`, `single`, `ep`, `compilation`… |
| Relations | how one track relates to another: `remix_of`, `instrumental_of`…; each reads both ways (`remix_of` ↔ `remix_to`), with a sentence for each ("%A is a remix of %B") |
| Review flags | the kinds of flag in Review. Some are the server's own checks (no cover, no title…); others are raised by hand (`Fix later`) or made by editors in the track editor |
| Extra kinds | `vocals`, `stems`, `video`… (what an extra is) |
| Version words | words in a title that make it another version of a song, so "Song" and "Song (Instrumental)" are not taken for duplicates. Each has a *mark* (what versions with it have in common: `remastered` and `remaster` are both "remaster"); a word can be found anywhere in a title, for languages written without spaces |

## Locked words

A few words the app is built around are **locked**: `primary`, `featured` and `producer`
(roles), `released` (status), `album` and `single` (album kinds), `video` and `other` (extras),
the automatic review checks, and the relations the editor's version tools use (remix,
instrumental, acapella, alternate). A locked word can be described and ordered, but not
renamed or removed, like the word "artist": the code assumes it. Everything else is the
library's own.

## Changing the others

* **Add** one at the bottom of its list. A relation is added with the word for the other
  direction (the same word if it reads the same both ways) and its sentence.
* **Rename** one and everything that uses it follows (a status renamed, and the tracks have
  the new name). Old entries in the history keep the name they were made with.
* **Remove** one only when nothing uses it: the list says how many things do.

Forms in the apps pick up a change at once (the desktop asks again when you save; the phone
at its next start).

## For scripts

`GET /api/vocab` is what forms read (the words of each list). `GET /api/vocabulary` is the
whole table, with how many rows use each word; `POST /api/vocabulary`, `PUT
/api/vocabulary/{list}/{word}` and `DELETE` change it (admins).
