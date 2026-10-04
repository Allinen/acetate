# Configuration

The server reads environment variables (and a `.env` file, if present).

| Variable | Default | |
|---|---|---|
| `DATABASE_URL` | *required* | Postgres connection string |
| `ACETATE_BIND` | `127.0.0.1:3000` | address to listen on |
| `ACETATE_VOLUMES` | one volume `main` | `name=path,name=path`. The first is default |
| `ACETATE_DATA_DIR` | `./data` | the single volume when `ACETATE_VOLUMES` is unset |
| `ACETATE_CACHE_DIR` | `cache/` in the first volume | smaller cover copies; safe to empty |
| `ACETATE_WEB_DIR` | `./web` | the web UI's files |
| `ACETATE_MAX_UPLOAD_MB` | `100` | per upload request; 0 for no limit |
| `ACETATE_UPLOAD_DAILY_GB` | `20` | all accounts together, per day; 0 for no limit |
| `ACETATE_SECURE_COOKIES` | off | `true` when serving over HTTPS (breaks plain HTTP logins) |
| `ACETATE_TRUST_PROXY` | off | `true` only if a proxy always sets `X-Forwarded-For` |

Imports on the server itself (`acetate ingest`, `import`) don't go through the API, so
upload limits don't apply to them.

The Docker setup uses `deploy/.env`: see `deploy/.env.example` for `POSTGRES_PASSWORD`,
`POSTGRES_DATA`, `FAST_DISK`, `SLOW_DISK`, `ACETATE_UID`/`ACETATE_GID`, and `ACETATE_PORT`.
