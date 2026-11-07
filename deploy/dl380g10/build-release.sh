#!/usr/bin/env bash
set -euo pipefail
umask 077
cd "$(dirname "$0")/../.."
output=${1:?usage: build-release.sh ABSOLUTE_NEW_ARTIFACT_DIRECTORY EXACT_SOURCE_SHA}
selected_head=${2:?exact selected source SHA is required}
[[ $output = /* && ! -e $output ]]
[[ $selected_head =~ ^[0-9a-f]{40}$ ]]
[[ $(git rev-parse 'HEAD^{tree}') = "$(git rev-parse "$selected_head^{tree}")" ]]
[[ -z $(git status --porcelain) ]]
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}
export OMP_NUM_THREADS=1 MKL_NUM_THREADS=1 OPENBLAS_NUM_THREADS=1
cargo build --release --locked -p tl3-controller --bins --example tl3-deploy
mkdir -m 0700 "$output"
target=${CARGO_TARGET_DIR:-$PWD/target}
install -m 0700 "$target/release/tl3" "$target/release/tl3d" "$target/release/tl3-worker-supervisor" "$target/release/examples/tl3-deploy" "$output/"
(cd "$output" && sha256sum tl3 tl3d tl3-deploy tl3-worker-supervisor > SHA256SUMS)
python3 - "$output" "$selected_head" <<'PY'
import hashlib
import json
from pathlib import Path
import subprocess
import sys

output = Path(sys.argv[1])
head = sys.argv[2]
if subprocess.check_output(["git", "status", "--porcelain"]).strip():
    raise SystemExit("release provenance requires a clean source checkout")
names = ("tl3", "tl3d", "tl3-deploy", "tl3-worker-supervisor")
(output / "SOURCE.json").write_text(json.dumps({"head": head, "binaries": {
    name: hashlib.sha256((output / name).read_bytes()).hexdigest() for name in names}}, sort_keys=True))
PY
cat "$output/SHA256SUMS"
printf 'Release artifacts: %s\n' "$output"
