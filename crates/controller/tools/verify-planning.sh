#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
cargo test -p tl3-controller -p tl3-store --tests --locked
cd workers
/home/rob/.local/bin/uv run --locked --no-editable --python 3.14+gil pytest -q
