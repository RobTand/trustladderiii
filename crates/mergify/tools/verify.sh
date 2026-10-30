#!/usr/bin/env bash
# Run only inside an admitted PrismaBuild action.
set -euo pipefail
cargo fmt --manifest-path crates/mergify/Cargo.toml -- --check
cargo clippy --manifest-path crates/mergify/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path crates/mergify/Cargo.toml --locked
cargo test --manifest-path crates/github/Cargo.toml --locked
