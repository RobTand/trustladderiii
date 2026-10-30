#!/usr/bin/env bash
set -euo pipefail
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}
export RUST_TEST_THREADS=2
export OMP_NUM_THREADS=1 MKL_NUM_THREADS=1 OPENBLAS_NUM_THREADS=1
cargo test --locked -p tl3-config
cargo build --locked -p tl3-controller --bins --example tl3-deploy
export TL3_DEPLOY_HELPER="$PWD/target/debug/examples/tl3-deploy"
python3 -m unittest discover -s deploy/dl380g10 -p 'test_*.py' -v
if [[ -n ${TL3_DEPLOY_ARTIFACTS:-} ]]; then
  mkdir -p "$TL3_DEPLOY_ARTIFACTS"
  install -m 0700 target/debug/tl3 target/debug/tl3d target/debug/examples/tl3-deploy "$TL3_DEPLOY_ARTIFACTS/"
  sha256sum "$TL3_DEPLOY_ARTIFACTS/tl3" "$TL3_DEPLOY_ARTIFACTS/tl3d" "$TL3_DEPLOY_ARTIFACTS/tl3-deploy"
fi
