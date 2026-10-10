# Offline

The apps keep a copy of the library on your device, so they keep working when the server
can't be reached: on a train, or while the server is down. This page says what works, what
waits for the server, and what needs it.

## Knowing it's offline

When a request can't reach the server, the app counts as offline. The desktop says so
under the library line in the sidebar ("Offline for 5 minutes"); the phone shows a bar
above the player. While offline, the app doesn't wait on the server for each thing: what
needs it says so at once.

The app asks the server again now and then (after a few seconds, then less often, up to
once a minute), and at once when you press **Try now**, or when the phone joins a network.
Any answer from the server, even an error, counts as back online.

Back online, the app sends what waited (below), asks the library what changed, and asks
what your account may do. On the desktop a [mirror](library/mirror.md) that couldn't reach
the server tries again.

## What works offline

- **Browsing, searching, sorting and filtering:** the whole library is kept on your device,
  with each track's lyrics. Your playlists are kept too: after each library refresh, any
  playlist that changed is fetched in the background.
- **Playing what's on your device:** [downloads](android.md#keeping-music-on-the-phone), and
  on the desktop the [mirror](library/mirror.md). Tracks that aren't here are dimmed in the
  lists and the queue. Playing one starts the next one that is here instead ("Not on this
  computer: skipped 2"), and playback goes on past them the same way.
- **Covers:** from the downloads and the mirror, besides those already seen.
- **Lyrics** on Now playing.
- **The queue**, resuming where you were, and `acetate://` links.

## What waits for the server

These show at once, as if the server had them, and wait in an *outbox* on your device. They
are sent in the order you made them when the server is back. The outbox is kept on disk, so
quitting the app loses nothing. While offline, the desktop's sidebar and the phone's bar
say how many changes wait.

- **Stars and ratings.** Changing the same thing twice sends only the last change; starring
  and then unstarring it sends nothing.
- **Plays**, with when they happened. A play is counted once, even if it has to be sent
  twice. The server takes plays up to 90 days old.
- **Tracks added to a playlist.** The desktop shows them as *waiting* in the playlist's
  Added column until the server has them.

If the same thing was changed elsewhere meanwhile (on another device), your change still
wins, as it's the latest sent, but you're told: "“X” had been starred elsewhere meanwhile;
your change replaced it."

If the server refuses a change when it's sent (the track is gone, or the playlist isn't
there or isn't yours anymore), the change is dropped and you're told why. A star or rating
goes back to what it was.

## What needs the server

These are turned off while offline, each saying "Offline: … needs the server":

- **Editing:** edit mode turns off when the server goes away. Anything not yet saved in an
  open form is kept, and edit mode comes back on when the server is back. Review, the trash
  and uploads too.
- **Share links.**
- **Changing playlists** other than adding tracks: making, renaming or describing one,
  taking tracks out, reordering, moving one to the trash, its history and undo.
- **The server's settings** (admins).

Changing the server's address (Settings, Account) works offline: it's how to reach a server
that moved.
