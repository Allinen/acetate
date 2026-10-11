#!/usr/bin/env bash
# Prepares a release: the version everywhere, a commit, a tag. Pushing is yours.
#
#   scripts/release.sh 0.2.0
#
# Needs: a clean tree, on main, and git-cliff (`cargo install git-cliff --locked`). It runs
# the tests (the server's against the dev database), drafts CHANGELOG.md's entry for the
# release from the commits since the last one (cliff.toml) and opens it in $EDITOR: write
# it for the people who use Acetate, not for the commit log. That entry is what the release
# page shows. Then it sets the version with scripts/versions.py, commits "release: 0.2.0"
# and tags v0.2.0. Then:
#
#   git push origin main     (wait for CI), then run the Release workflow by hand on main
#   (a dry run: it builds everything and publishes nothing), then:
#   git push origin v0.2.0
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

echo "== changelog"
command -v git-cliff > /dev/null || { echo "git-cliff isn't installed: cargo install git-cliff --locked" >&2; exit 1; }
if grep -q "^## $version " CHANGELOG.md; then
    echo "CHANGELOG.md has an entry for $version already: using it as it is"
else
    draft="$(mktemp)"
    git cliff --unreleased --tag "$tag" --strip header 2> /dev/null > "$draft"
    # The draft goes under the "# Changelog" heading.
    { head -n 2 CHANGELOG.md; cat "$draft"; echo; tail -n +3 CHANGELOG.md; } > "$draft.new"
    mv "$draft.new" CHANGELOG.md
    rm -f "$draft"
    "${VISUAL:-${EDITOR:-vi}}" CHANGELOG.md
    grep -q "^## $version " CHANGELOG.md || { echo "CHANGELOG.md has no '## $version' entry: nothing done to the version (restore it with git checkout CHANGELOG.md)" >&2; exit 1; }
fi

echo "== version"
scripts/versions.py --set "$version"
git add -A
# (Nothing to commit when the version is the one already there: the first release.)
git diff --cached --quiet || git commit -q -m "release: $version"
git tag -a "$tag" -m "Acetate $version"

echo
echo "done: commit and tag $tag are here, nothing is pushed. To release:"
echo "  1. git push origin main            (CI runs on main; wait for it)"
echo "  2. Actions -> Release -> Run workflow, branch main: builds the apps and the image without"
echo "     releasing anything. Fix what fails (and move the tag: git tag -f $tag) before step 3."
echo "  3. git push origin $tag            (only now: this publishes)"
