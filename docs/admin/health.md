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
| Files are undamaged | Re-reads every file in use and compares its content with its name (the SHA-256). It reads the whole library, so it is not ticked by default. |

The first three are quick. `acetate verify --blobs` runs the same checks from the command
line (it also reads every file). To find files in an old folder that Acetate doesn't have,
see `acetate verify <dir>` in [Command line](cli.md).

More checks can be added over time; a new one appears on this page by itself.
