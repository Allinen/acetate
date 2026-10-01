#!/usr/bin/env bash
# Builds the demo library for screenshots: (re)creates the database acetate_demo_dev,
# creates the user admin / adminadmin and imports ./demo-audio (generated first by
# scripts/make-demo-library.py if it doesn't exist yet). Blobs go to ./demo-data, so
# none of it mixes with the dev library; delete both folders to get rid of it.
#
#   scripts/reset-demo-db.sh
#
# Then run the server on it:
#
#   DATABASE_URL=postgres://acetate:acetate@127.0.0.1:5432/acetate_demo_dev \
#   ACETATE_DATA_DIR=./demo-data cargo run -- serve
#
# The database is dropped even while sessions are connected (a running server's included).
set -euo pipefail

cd "$(dirname "$0")/.."

db=acetate_demo_dev
export DATABASE_URL="postgres://acetate:acetate@127.0.0.1:5432/$db"
export ACETATE_DATA_DIR=./demo-data

[[ -d demo-audio ]] || scripts/make-demo-library.py

# Built from .sqlx/: the demo database doesn't have the tables to check the queries yet.
SQLX_OFFLINE=true cargo build -q

echo "recreating database '$db'..."
docker compose exec -T postgres psql -q -v ON_ERROR_STOP=1 -U acetate -d postgres <<SQL
SET client_min_messages = warning;
DROP DATABASE IF EXISTS $db WITH (FORCE);
CREATE DATABASE $db;
SQL

# Every acetate command applies the migrations first, so this one also creates the tables.
echo "creating user 'admin' (password 'adminadmin')..."
printf 'adminadmin\n' | ./target/debug/acetate user create admin --group admin --password-stdin

echo "importing demo-audio..."
./target/debug/acetate ingest demo-audio --layout tags --origin demo

# Where the files came from shows in the app (e.g. on the Review page): a neutral path
# instead of this checkout's, for screenshots.
docker compose exec -T postgres psql -q -U acetate -d "$db" -c \
    "UPDATE files SET imported_from = replace(imported_from, '$PWD/demo-audio/', '/music/')"

echo "done."
