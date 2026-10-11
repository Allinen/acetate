# Configuration

The server reads environment variables (and a `.env` file, if present).

| Variable | Default | |
|---|---|---|
| `DATABASE_URL` | *required* | Postgres connection string |
| `ACETATE_BIND` | `127.0.0.1:3000` | address to listen on |
| `ACETATE_VOLUMES` | one volume `main` | `name=path,name=path`. The first is default |
| `ACETATE_DATA_DIR` | `./data` | the single volume when `ACETATE_VOLUMES` is unset |
| `ACETATE_CACHE_DIR` | `cache/` in the first volume | smaller cover copies and transcoded audio; safe to empty |
| `ACETATE_TRANSCODE_KBPS` | `128` | the bitrates (24–510) of the Opus copies listeners can get, e.g. `128,64,192`; the first is the default. Nothing else can be asked for |
| `ACETATE_TRANSCODE_CACHE_GB` | `20` | size limit of the transcoded copies (the least recently played go first), all bitrates together; 0 for no limit |
| `ACETATE_WEB_DIR` | `./web` | the web UI's files |
| `ACETATE_MAX_UPLOAD_MB` | `100` | per upload request; 0 for no limit |
| `ACETATE_UPLOAD_DAILY_GB` | `20` | all accounts together, per day; 0 for no limit |
| `ACETATE_YTDLP_COOKIES` | none | path to a `cookies.txt` (Netscape format) exported from a logged-in browser, given to yt-dlp when YouTube answers "Sign in to confirm you're not a bot". yt-dlp updates the file, so it must be writable (in Docker, mount it, not just its folder read-only) |
| `ACETATE_SECURE_COOKIES` | off | `true` (or `1`, `yes`, `on`; `false`, `0`, `no`, `off`; anything else stops the server) when serving over HTTPS (breaks plain HTTP logins) |
| `ACETATE_TRUST_PROXY` | off | `true` only if a proxy always sets `X-Forwarded-For` |

Transcoding needs `ffmpeg` (with libopus) on the server; the Docker image has it. Without it,
listeners who aren't allowed the original can't play (the original is never sent instead).

Imports on the server itself (`acetate ingest`, `import`) don't go through the API, so
upload limits don't apply to them.

The Docker setup uses `deploy/.env`: see `deploy/.env.example` for `POSTGRES_PASSWORD`,
`POSTGRES_DATA`, `DISK` (and optionally `DISK2`), `ACETATE_VOLUMES`, `ACETATE_UID`/`ACETATE_GID`,
`ACETATE_IMAGE` and `ACETATE_PORT`.
