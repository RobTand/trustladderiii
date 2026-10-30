#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo fmt --all -- --check
cargo clippy --locked -p tl3-github -p tl3-mergify -p tl3-controller --all-targets -- -D warnings
cargo test --locked -p tl3-github -p tl3-mergify --all-targets
cargo test --locked -p tl3-controller --test mergify
cargo test --locked -p tl3-controller --test sandbox_gate
python3 -m unittest discover -s deploy/sandbox -p 'test_*.py' -v
