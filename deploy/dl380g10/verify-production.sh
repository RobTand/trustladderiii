#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}
export RUST_TEST_THREADS=2
export OMP_NUM_THREADS=1 MKL_NUM_THREADS=1 OPENBLAS_NUM_THREADS=1
cargo test --locked -p tl3-controller --all-targets
bash deploy/dl380g10/verify.sh
