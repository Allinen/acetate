# Moving from Navidrome

1. Ingest your tracks: `acetate ingest <music dir> --layout tags --origin <label>`.
2. Import the users, plays, stars and ratings:

   ```sh
   acetate import-navidrome --db navidrome.db --path-map /navidrome/music=/acetate/music --dry-run
   ```

   Drop `--dry-run` when the report looks right.

- **Parts:** `--only users,annotations,playlists,scrobbles` brings over just those
  (default: all). `scrobbles` is the play log with times (Navidrome 0.55+); `annotations`
  are counts, stars and ratings. Without `users`, Navidrome's users are matched to existing
  Acetate ones by name, and any others are skipped. Running again never doubles anything.
- **Users:** each becomes an Acetate user with the same username (admins go in `admin`, the
  rest in `user`).
- **Matching** is by path, then by content, so relocated files are still found. Anything
  unsure is reported, not guessed.
- **Playlists:** regular ones are imported; smart playlists aren't yet.
- Navidrome's database and files are only read, never changed.
- `--path-map` is repeatable: it says where Navidrome's paths are on this machine (Docker
  mounts). `--library` limits to one Navidrome library.

Acetate has no Subsonic API, so existing Subsonic clients won't connect: use the desktop
app or web UI.
