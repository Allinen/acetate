#!/usr/bin/env bash
# Prepares a release: the version everywhere, a commit, a tag. Pushing is yours.
#
#   scripts/release.sh 0.2.0
#
# Needs: a clean tree, on main. It runs the tests (the server's against the dev database),
# sets the version with scripts/versions.py, commits "release: 0.2.0" and tags v0.2.0. Then:
#
#   git push origin main v0.2.0
#
# and the Release workflow builds the apps and the image from the tag (it checks the tag
# against the versions again). Look at API_VERSION and CATALOG_FORMAT first: they are
# printed below, and change only when a change would break an app that doesn't have it.
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
cd "$(dirname "$0")/.."

version="${1:-}"
version="${version#v}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || { echo "usage: scripts/release.sh 1.2.3" >&2; exit 1; }
tag="v$version"

[ "$(git rev-parse --abbrev-ref HEAD)" = main ] || { echo "not on main" >&2; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "the tree isn't clean" >&2; git status --short >&2; exit 1; }
! git rev-parse -q --verify "refs/tags/$tag" > /dev/null || { echo "$tag exists already" >&2; exit 1; }

echo "releasing $tag (API_VERSION $(cat API_VERSION), CATALOG_FORMAT $(cat CATALOG_FORMAT)); last release: $(git describe --tags --abbrev=0 2>/dev/null || echo none)"

echo "== tests"
SQLX_OFFLINE=true cargo clippy --all-targets --quiet -- -D warnings
(cd clients && cargo clippy --workspace --all-targets --quiet -- -D warnings)
cargo test --quiet
(cd clients && cargo test --workspace --quiet)

echo "== version"
scripts/versions.py --set "$version"
git add -A
# (Nothing to commit when the version is the one already there: the first release.)
git diff --cached --quiet || git commit -q -m "release: $version"
git tag -a "$tag" -m "Acetate $version"

echo
echo "done: commit and tag $tag are here, nothing is pushed. To release:"
echo "  git push origin main $tag"
