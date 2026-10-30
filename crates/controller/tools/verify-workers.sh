#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
/home/rob/.local/bin/uv sync --locked --project workers --python 3.14+gil
cargo fmt --all -- --check
cargo clippy -p tl3-controller --all-targets --locked -- -D warnings
cargo test -p tl3-controller --test workers --locked
