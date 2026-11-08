#!/usr/bin/env bash
# Frees disk space used by Cargo build artifacts in both workspaces (the server in the
# repo root, the clients in ./clients). Build output is fully regenerable; the cost is a
# slower next build.
#
#   scripts/clean-target.sh          # remove artifacts unused for 14 days (needs cargo-sweep)
#   scripts/clean-target.sh --all    # remove the whole dev profile (no extra tools needed)
#
# Install cargo-sweep with `cargo install cargo-sweep`. Without it, the default mode falls
# back to --all.
set -euo pipefail

cd "$(dirname "$0")/.."

mode=sweep
[[ "${1:-}" == "--all" ]] && mode=all
if [[ $mode == sweep ]] && ! command -v cargo-sweep >/dev/null; then
    echo "cargo-sweep not installed; falling back to --all" >&2
    mode=all
fi

echo "before:"
du -sh target clients/target 2>/dev/null || true

for dir in . clients; do
    if [[ $mode == all ]]; then
        (cd "$dir" && cargo clean --profile dev)
    else
        (cd "$dir" && cargo sweep --time 14)
    fi
done

echo "after:"
du -sh target clients/target 2>/dev/null || true
