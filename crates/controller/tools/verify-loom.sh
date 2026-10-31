#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
cargo test -p tl3-controller --features loom --lib --locked loom_ -- --test-threads=1

if cargo check -p tl3-controller --features loom --lib --locked > loom-feature-build.log 2>&1; then
    echo "loom feature unexpectedly built outside unit tests" >&2
    exit 1
fi
command -v grep >/dev/null
grep -F "the loom feature is only supported for library unit tests" loom-feature-build.log
