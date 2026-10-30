#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo build -p tl3-kernel --no-default-features --release --locked
