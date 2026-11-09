#!/usr/bin/env bash
# Run inside a PrismaBuild action, or on free remote compute.
set -euo pipefail
cd "$(dirname "$0")/../../.."
cargo test --locked -p tl3-controller --test direct_merge
cargo test --locked -p tl3-github --test rest required_status_checks
