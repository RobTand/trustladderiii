#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
cargo test -p tl3-controller --features loom --lib --locked loom_ -- --test-threads=1
