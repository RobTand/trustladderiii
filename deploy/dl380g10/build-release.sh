#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
output=${1:?usage: build-release.sh ABSOLUTE_NEW_ARTIFACT_DIRECTORY}
[[ $output = /* && ! -e $output ]]
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}
export OMP_NUM_THREADS=1 MKL_NUM_THREADS=1 OPENBLAS_NUM_THREADS=1
cargo build --release --locked -p tl3-controller --bins --example tl3-deploy
mkdir -m 0700 "$output"
target=${CARGO_TARGET_DIR:-$PWD/target}
install -m 0700 "$target/release/tl3" "$target/release/tl3d" "$target/release/examples/tl3-deploy" "$output/"
(cd "$output" && sha256sum tl3 tl3d tl3-deploy > SHA256SUMS)
cat "$output/SHA256SUMS"
printf 'Release artifacts: %s\n' "$output"
