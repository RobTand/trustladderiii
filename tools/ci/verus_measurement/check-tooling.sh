#!/bin/bash
# Run only inside an admitted PB action. No verifier execution.
set -uo pipefail
export OMP_NUM_THREADS=1 OPENBLAS_NUM_THREADS=1 MKL_NUM_THREADS=1 RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1
result=0
python3 -I tools/ci/verus_measurement/fixture-check.py || result=1
cargo fmt --package tl3-verus-runner || result=1
python3 -I -c 'import base64,pathlib; print("FORMATTED_RUNNER_BASE64 " + base64.b64encode(pathlib.Path("crates/verus-runner/src/bin/verus-runner.rs").read_bytes()).decode())'
cargo clippy --locked --package tl3-verus-runner --bin verus-runner -- -D warnings || result=1
exit "$result"
