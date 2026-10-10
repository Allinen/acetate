#!/usr/bin/env bash
# Uses the hooks in .githooks/ (pre-commit: versions and generated docs; pre-push: clippy).
set -euo pipefail
cd "$(dirname "$0")/.."
git config core.hooksPath .githooks
echo "hooks installed (git config core.hooksPath .githooks)"
