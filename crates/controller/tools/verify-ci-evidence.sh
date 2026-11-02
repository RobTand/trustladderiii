#!/usr/bin/env bash
# Run inside a PrismaBuild action, or on free remote compute.
set -euo pipefail
cd "$(dirname "$0")/../../.."
cargo test -p tl3-controller --test mergify --locked -- --ci-evidence
cargo test -p tl3-github --test rest --locked ci_
