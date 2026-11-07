#!/bin/bash
# Run only inside an admitted PB action. No verifier execution.
set -euo pipefail
export OMP_NUM_THREADS=1 OPENBLAS_NUM_THREADS=1 MKL_NUM_THREADS=1 RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1
python3 -I tools/ci/verus_measurement/fixture-check.py
cargo fmt --package tl3-verus-runner -- --check
cargo clippy --locked --package tl3-verus-runner --bin verus-runner -- -D warnings
