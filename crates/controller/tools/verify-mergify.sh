#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
cargo fmt --all -- --check
cargo clippy -p tl3-controller -p tl3-github -p tl3-mergify --all-targets --locked -- -D warnings
cargo test -p tl3-controller -p tl3-github -p tl3-mergify --all-targets --locked
