#!/usr/bin/env bash
set -uo pipefail
cd "$(dirname "$0")/../../.."
status=0
cargo test --locked -p tl3-worktrees scope_ -- --nocapture || status=1
cargo test --locked -p tl3-controller --lib scope_ -- --nocapture || status=1
cargo test --locked -p tl3-controller --test scope -- --nocapture || status=1
cargo test --locked -p tl3-controller --test writer_quote scope_ -- --nocapture || status=1
"${TL3_TEST_UV:-/home/rob/.local/bin/uv}" run --locked --project workers --python 3.14+gil pytest -q workers/tests/test_reviewer.py workers/tests/test_writer.py -k scope_ || status=1
exit "$status"
