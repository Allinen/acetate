# Health checks

Settings → Health (admins) runs checks on the library and its files. They only look:
nothing is changed or deleted, and what a finding means is yours to decide. Pick the checks,
press **Run the checks**; the server runs them in the background (one run at a time) and the
page shows the progress and, afterwards, the report. The last report is kept until the server
restarts.

| Check | Looks for |
|---|---|
| Files are where the database says | A file the library uses that is in no volume (a problem), or is on a different volume than recorded, or on two volumes (notes). Reads folder listings only. |
| Files nothing uses | Files in the blob store that no track, cover, extra or unimported file refers to: a failed upload, a crash while purging. Images waiting in the editor aren't counted. |
| Staged cover images nobody used | Images put into the editor a day or more ago that no edit applied. |
| Tracks that play a video file | Tracks made from a video extra before the sound was taken out of videos: they play the video itself, which only works where the player can decode its container. |
| Files are undamaged | Re-reads every file in use and compares its content with its name (the SHA-256). It reads the whole library, so it is not ticked by default. |

The first four are quick. `acetate verify --blobs` runs the same checks from the command
line (it also reads every file). To find files in an old folder that Acetate doesn't have,
see `acetate verify <dir>` in [Command line](cli.md).

**Tracks that play a video file** is the one finding with a repair, and it is its own button, never part of a run: *See what it would do* lists the tracks, and *Take the sound out* copies each video's sound out as it is (nothing is re-encoded; needs ffmpeg on the server) and makes it the track's file. The video file stays as a previous file, and the video extra is shown again and linked to the track. A track that can't be repaired (no ffmpeg, a sound that can't be copied as it is) is left as it was, with the reason. Running it again does nothing for the tracks already done.

More checks can be added over time; a new one appears on this page by itself.
