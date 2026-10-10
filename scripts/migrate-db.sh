#!/usr/bin/env bash
# Applies the migrations that haven't run yet to a local database (the dev one, say, after
# pulling migrations someone else wrote), and nothing else.
#
#   scripts/migrate-db.sh
#
# Uses DATABASE_URL (from the environment or .env) and refuses anything that isn't on this
# machine (a server applies its own migrations when it starts). Uses sqlx-cli if it's
# installed, else builds against the query cache checked in (.sqlx): a plain `cargo build`
# checks the queries against the database and fails until it's migrated.
set -euo pipefail

cd "$(dirname "$0")/.."

case "${1:-}" in
    "") ;;
    -h|--help) sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
esac

if [[ -z "${DATABASE_URL:-}" && -f .env ]]; then
    DATABASE_URL=$(grep -E '^DATABASE_URL=' .env | tail -n1 | cut -d= -f2-)
fi
: "${DATABASE_URL:?DATABASE_URL is not set, in the environment or .env}"
export DATABASE_URL

if [[ ! "$DATABASE_URL" =~ ^postgres(ql)?://([^@/]+@)?([^:/]+)(:[0-9]+)?/([^?]+) ]]; then
    echo "can't read DATABASE_URL: $DATABASE_URL" >&2
    exit 1
fi
db_host="${BASH_REMATCH[3]}"
db_name="${BASH_REMATCH[5]}"
case "$db_host" in
    127.0.0.1|localhost|::1) ;;
    *) echo "refusing to migrate a database on $db_host (only local ones)" >&2; exit 1 ;;
esac

echo "migrating database '$db_name'..."
# sqlx-cli needs only the migration files; without it, the server is built (against the
# query cache, which a query new in the working tree isn't in yet: then install sqlx-cli,
# `cargo install sqlx-cli --no-default-features --features postgres`).
if command -v sqlx >/dev/null; then
    sqlx migrate run
else
    SQLX_OFFLINE=true cargo build -q
    ./target/debug/acetate migrate
fi
