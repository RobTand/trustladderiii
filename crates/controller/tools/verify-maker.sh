#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
"${TL3_TEST_UV:-/home/rob/.local/bin/uv}" tool run --from ruff==0.14.14 ruff check --isolated --select E9,F \
  crates/controller/tests crates/controller/tools workers/src workers/tests
bash crates/controller/tools/verify.sh
"${TL3_TEST_UV:-/home/rob/.local/bin/uv}" run --locked --project workers --python 3.14+gil pytest -c workers/pyproject.toml workers/tests -q
"${TL3_TEST_UV:-/home/rob/.local/bin/uv}" run --locked --project workers --python 3.14+gil pytest -c harness/pyproject.toml harness/tests -q
