#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
TL3_LOOM=1 cargo test -p tl3-controller --lib --locked loom_ -- --test-threads=1
