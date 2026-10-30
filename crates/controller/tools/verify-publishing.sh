#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
cargo test -p tl3-controller --test publishing --test github --test prismabuild --locked
cd workers
"${TL3_TEST_UV:-uv}" run --locked --no-editable --python 3.14+gil pytest -q
