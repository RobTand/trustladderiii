#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
cargo test -p tl3-controller --test publishing --locked
cargo test --locked --all-targets -p tl3-controller -p tl3-store -p tl3-github -p tl3-worktrees -p tl3-prismabuild -p tl3-mergify
cd workers
/home/rob/.local/bin/uv run --locked --no-editable --python 3.14+gil pytest -q
