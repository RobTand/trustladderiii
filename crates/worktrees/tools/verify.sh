#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
cargo fmt --check
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
