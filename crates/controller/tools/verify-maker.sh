#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
bash crates/controller/tools/verify.sh
"${TL3_TEST_UV:-/home/rob/.local/bin/uv}" run --locked --project workers --python 3.14+gil pytest -c workers/pyproject.toml workers/tests -q
"${TL3_TEST_UV:-/home/rob/.local/bin/uv}" run --locked --project workers --python 3.14+gil pytest -c harness/pyproject.toml harness/tests -q
