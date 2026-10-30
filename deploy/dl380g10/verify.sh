#!/usr/bin/env bash
set -euo pipefail
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}
export RUST_TEST_THREADS=2
export OMP_NUM_THREADS=1 MKL_NUM_THREADS=1 OPENBLAS_NUM_THREADS=1
cargo test --locked -p tl3-config
cargo test --locked -p tl3-controller --example tl3-deploy
cargo build --locked -p tl3-controller --bins --example tl3-deploy
cargo_target=${CARGO_TARGET_DIR:-$PWD/target}
export TL3_DEPLOY_HELPER="$cargo_target/debug/examples/tl3-deploy"
python3 -m unittest discover -s deploy/dl380g10 -p 'test_*.py' -v
if [[ -n ${TL3_DEPLOY_ARTIFACTS:-} ]]; then
  mkdir -p "$TL3_DEPLOY_ARTIFACTS"
  install -m 0700 "$cargo_target/debug/tl3" "$cargo_target/debug/tl3d" "$TL3_DEPLOY_HELPER" "$TL3_DEPLOY_ARTIFACTS/"
  sha256sum "$TL3_DEPLOY_ARTIFACTS/tl3" "$TL3_DEPLOY_ARTIFACTS/tl3d" "$TL3_DEPLOY_ARTIFACTS/tl3-deploy"
fi
