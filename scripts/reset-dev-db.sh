#!/usr/bin/env bash
# Resets the dev database to empty: drops every table, reapplies the migrations and
# creates an admin user. With --with-audio, also imports ./test-audio (generated first if
# it doesn't exist yet).
#
#   scripts/reset-dev-db.sh                # empty library, user admin / adminadmin
#   scripts/reset-dev-db.sh --with-audio   # plus the test files
#
# Uses DATABASE_URL (from the environment or .env) and refuses anything that isn't on this
# machine, or whose name doesn't end in _dev, _test or _scratch. Blobs in the data dir are
# left alone: they're content-addressed, so importing the same file again reuses them.
#
# Unlike `sqlx database reset`, this works while other sessions (psql, the server) are
# connected; it drops the schema instead of the whole database. Restart a running server
# afterwards: its connections still remember the old tables.
set -euo pipefail

cd "$(dirname "$0")/.."

with_audio=false
for arg in "$@"; do
    case "$arg" in
        --with-audio) with_audio=true ;;
        -h|--help) sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option: $arg" >&2; exit 2 ;;
    esac
done

if [[ -z "${DATABASE_URL:-}" && -f .env ]]; then
    DATABASE_URL=$(grep -E '^DATABASE_URL=' .env | tail -n1 | cut -d= -f2-)
fi
: "${DATABASE_URL:?DATABASE_URL is not set, in the environment or .env}"
export DATABASE_URL

# postgres://user:password@host:port/database
if [[ ! "$DATABASE_URL" =~ ^postgres(ql)?://([^:@/]+)(:[^@]*)?@([^:/]+)(:[0-9]+)?/([^?]+) ]]; then
    echo "can't read DATABASE_URL: $DATABASE_URL" >&2
    exit 1
fi
db_user="${BASH_REMATCH[2]}"
db_host="${BASH_REMATCH[4]}"
db_name="${BASH_REMATCH[6]}"

case "$db_host" in
    127.0.0.1|localhost|::1) ;;
    *) echo "refusing to reset a database on $db_host (only local ones)" >&2; exit 1 ;;
esac

# Only databases whose name says they're disposable: the real library may live in the same
# Postgres (e.g. `acetate`), one wrong DATABASE_URL away.
case "$db_name" in
    *_dev|*_test|*_scratch) ;;
    *) echo "refusing to reset '$db_name': only databases named *_dev, *_test or *_scratch" >&2; exit 1 ;;
esac

admin_user="${ACETATE_DEV_ADMIN:-admin}"
admin_password="${ACETATE_DEV_PASSWORD:-adminadmin}"

# Build first: sqlx checks the queries against the database while compiling, which
# can't work once the tables are gone.
cargo build -q

echo "resetting database '$db_name'..."
# lock_timeout: a session in the middle of a transaction (e.g. psql after BEGIN) would
# otherwise make this wait forever.
if ! docker compose exec -T postgres psql -q -v ON_ERROR_STOP=1 -U "$db_user" -d "$db_name" <<'SQL'
SET lock_timeout = '5s';
SET client_min_messages = warning;
DROP SCHEMA public CASCADE;
CREATE SCHEMA public;
SQL
then
    echo "couldn't drop the tables; is another session in the middle of a transaction" \
         "(e.g. psql after BEGIN)? End it and try again." >&2
    exit 1
fi

# Every acetate command applies the migrations first, so this one also recreates the tables.
echo "creating user '$admin_user' (password '$admin_password')..."
printf '%s\n' "$admin_password" |
    ./target/debug/acetate user create "$admin_user" --group admin --password-stdin

if $with_audio; then
    if [[ ! -d test-audio ]]; then
        scripts/make-test-audio.sh
    fi
    echo "importing test-audio..."
    find test-audio -maxdepth 1 -type f ! -name '*.png' -print0 |
        xargs -0 ./target/debug/acetate import \
            --origin '{"platform":"custom","data":{"label":"test audio"}}'
fi

echo "done. Restart the server if it's running."
