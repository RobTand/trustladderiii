#!/usr/bin/env bash
# Run only inside an admitted PrismaBuild action.
set -euo pipefail
for crate in mergify github; do
  manifest="crates/$crate/Cargo.toml"
  cargo fmt --manifest-path "$manifest" -- --check
  cargo clippy --manifest-path "$manifest" --locked --all-targets -- -D warnings
  cargo test --manifest-path "$manifest" --locked
done
