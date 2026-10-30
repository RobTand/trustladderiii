#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
/home/rob/.local/bin/uv sync --locked --project workers --python 3.14+gil
bash crates/controller/tools/verify.sh
