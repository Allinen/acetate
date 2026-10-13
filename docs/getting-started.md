# Getting started

Acetate runs as two containers: Postgres and the server. The server's image is published
at `ghcr.io/allinen/acetate` (for amd64 and arm64), so all you need from a checkout is the
`deploy` folder.

```sh
git clone https://github.com/Allinen/acetate.git && cd acetate/deploy
cp .env.example .env        # set POSTGRES_PASSWORD, POSTGRES_DATA and DISK (where your music lives)
docker compose up -d
docker compose exec acetate acetate user create admin --group admin   # asks for a password
```

`.env` is explained in [Configuration](admin/configuration.md).

## Bring your music in

Paths are as the *container* sees them: with `DISK=/srv/media`, `/srv/media/Music` is
`/disk/Music`.

```sh
docker compose exec acetate acetate ingest /disk/Music --layout tags --origin my-library --dry-run
```

`--dry-run` prints what would happen without changing anything. Drop it once the output
looks right. See [Importing music](library/ingest.md) for layouts and options.

Keep your music on the same mount as Acetate's volume. Imports are then reflinks:
instant, and no extra disk space (on btrfs, XFS, or ZFS with block cloning; elsewhere the
files are copied).

## Install a client

- **Desktop app** (Linux, Windows): download it from the
  [releases page](https://github.com/Allinen/acetate/releases), or build it with
  `clients/desktop/install.sh`. This is the intended way to use Acetate. When the server is
  a newer release than the app, the sidebar (and Settings → General) has an **Update** button: it downloads
  the server's release from GitHub, checks it against the release's `.sha256` file, and replaces the program (restart to use it). It never
  goes back to an older one: if the server is older, the app only says so, and the server
  needs updating (an older server can't read what a newer app keeps).
- **Android app:** see [The Android app](android.md). Listening, playlists, search and downloads.
- **In a browser:** `http://<your-server>:3000` has only the page for share links, and this guide at `/docs`. Everything else is in the apps.

Log in with the user you created. Then press `?` to see the keys, and `Ctrl+K` to open the
command palette.

## Next

- [Listening](listening.md)
- [Moving from Navidrome](library/navidrome.md)
- [Users and permissions](admin/users.md)
