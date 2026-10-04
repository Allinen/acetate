# Getting started

Acetate runs as two containers: Postgres and the server. There is no published image yet,
so build it from a checkout.

```sh
git clone https://github.com/Allinen/acetate.git && cd acetate
docker build -t acetate .
cd deploy
cp .env.example .env        # set POSTGRES_PASSWORD, your disks, and ACETATE_IMAGE=acetate
docker compose up -d
docker compose exec acetate acetate user create admin --group admin   # asks for a password
```

`.env` is explained in [Configuration](admin/configuration.md).

## Bring your music in

Paths are as the *container* sees them: with `FAST_DISK=/mnt/fast`, `/mnt/fast/Music` is
`/pool/Music`.

```sh
docker compose exec acetate acetate ingest /pool/Music --layout tags --origin my-library --dry-run
```

`--dry-run` prints what would happen without changing anything. Drop it once the output
looks right. See [Importing music](library/ingest.md) for layouts and options.

Keep your music on the same mount as Acetate's volume. Imports are then reflinks:
instant, and no extra disk space (on btrfs, XFS, or ZFS with block cloning; elsewhere the
files are copied).

## Install a client

- **Desktop app** (Linux, Windows): download it from the
  [releases page](https://github.com/Allinen/acetate/releases), or build it with
  `clients/desktop/install.sh`. This is the intended way to use Acetate.
- **Web UI:** `http://<your-server>:3000`. Simpler, lacks many features - fine for visitors.

Log in with the user you created. Then press `?` to see the keys, and `Ctrl+K` to open the
command palette.

## Next

- [Listening](listening.md)
- [Moving from Navidrome](library/navidrome.md)
- [Users and permissions](admin/users.md)
