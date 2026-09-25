#!/usr/bin/env bash
# Builds Acetate's Docker image and pushes it to the registry.
#
#   scripts/build-image.sh          # :latest, plus a :YYYYMMDD-HHMM tag to roll back to
#   scripts/build-image.sh v0.1     # another tag instead of latest
#
# ACETATE_REGISTRY overrides the registry (default localhost:5000). The image is built
# without a database, from `.sqlx/`: after changing any query, run
# `cargo sqlx prepare -- --all-targets` first (with DATABASE_URL on an up-to-date dev DB).
set -euo pipefail

cd "$(dirname "$0")/.."

registry="${ACETATE_REGISTRY:-localhost:5000}"
tag="${1:-latest}"
stamp="$(date +%Y%m%d-%H%M)"

# The same check the image build does, but here it fails in seconds instead of minutes.
if ! SQLX_OFFLINE=true cargo check --release --quiet; then
    echo ".sqlx/ doesn't match the queries: run cargo sqlx prepare -- --all-targets" >&2
    exit 1
fi

docker build -t "$registry/acetate:$tag" -t "$registry/acetate:$stamp" .
docker push "$registry/acetate:$tag"
docker push "$registry/acetate:$stamp"
echo "pushed $registry/acetate:$tag and :$stamp"
