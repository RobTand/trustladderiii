#!/usr/bin/env bash
set -euo pipefail
umask 077
for key in ${!GIT_@}; do unset "$key"; done
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0
cd "$(dirname "$0")/../.."
output=${1:?usage: build-release.sh ABSOLUTE_NEW_ARTIFACT_DIRECTORY EXACT_SOURCE_SHA}
selected_head=${2:?exact selected source SHA is required}
[[ $output = /* && ! -e $output ]]
[[ $selected_head =~ ^[0-9a-f]{40}$ ]]
[[ $(timeout --kill-after=5s 30s git rev-parse HEAD) = "$selected_head" ]]
[[ $(timeout --kill-after=5s 30s git rev-parse 'HEAD^{tree}') = "$(timeout --kill-after=5s 30s git rev-parse "$selected_head^{tree}")" ]]
source_status=$(timeout --kill-after=5s 30s git status --porcelain)
[[ -z $source_status ]]
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}
export OMP_NUM_THREADS=1 MKL_NUM_THREADS=1 OPENBLAS_NUM_THREADS=1
artifacts=$(mktemp "${output}.artifacts.XXXXXXXX")
trap 'rm -f -- "$artifacts"' EXIT
cargo build --release --locked -p tl3-controller --bins --example tl3-deploy --message-format=json-render-diagnostics > "$artifacts"
# Canary-qualification staging is example-only (FX-REQUAL-43 OQ-1). Cargo's own artifact
# report must show the controller library and every shipped target built with it off.
python3 -I - "$artifacts" <<'PY'
import json
import sys

FORBIDDEN = 'activation-qualification'
SHIPPED = {('lib', 'tl3_controller'), ('bin', 'tl3'), ('bin', 'tl3d'), ('bin', 'tl3-run-backend'),
           ('bin', 'tl3-worker-supervisor'), ('example', 'tl3-deploy')}
seen = set()
with open(sys.argv[1], encoding='utf-8') as report:
    for line in report:
        if not line.startswith('{'):
            continue
        message = json.loads(line)
        if message.get('reason') != 'compiler-artifact':
            continue
        target = message['target']
        if FORBIDDEN in message.get('features', []):
            raise SystemExit(f'refusing release: {target["name"]} was built with {FORBIDDEN}')
        package = message['package_id']
        if package.split('#', 1)[-1].startswith('tl3-controller@') or package.startswith('tl3-controller '):
            seen.update((kind, target['name']) for kind in target['kind'])
missing = sorted(SHIPPED - seen)
if missing:
    raise SystemExit(f'refusing release: no Cargo artifact report for {missing}')
PY
rm -f -- "$artifacts"
# Publish only a complete bundle; a failed build/copy leaves no retry-wedging output.
staging=$(mktemp -d "${output}.staging.XXXXXXXX")
trap 'rm -rf -- "$staging"' EXIT
chmod 0700 "$staging"
target=${CARGO_TARGET_DIR:-$PWD/target}
# Second layer: refuse release bytes that carry the staging constructor or packet
# marker; a scan error also refuses. tl3-run-backend is built here though not installed.
for binary in "$target/release/tl3" "$target/release/tl3d" "$target/release/tl3-run-backend" "$target/release/tl3-worker-supervisor" "$target/release/examples/tl3-deploy"; do
  scan=0
  grep -aqF -e for_canary_qualification -e tl3.fx_canary_qualification_staging.v1 -- "$binary" || scan=$?
  if [[ $scan != 1 ]]; then
    printf 'refusing release: %s carries canary-qualification staging or cannot be scanned (%s)\n' "$binary" "$scan" >&2
    exit 1
  fi
done
install -m 0700 "$target/release/tl3" "$target/release/tl3d" "$target/release/tl3-worker-supervisor" "$target/release/examples/tl3-deploy" "$staging/"
(cd "$staging" && sha256sum tl3 tl3d tl3-deploy tl3-worker-supervisor > SHA256SUMS)
python3 -I - "$staging" "$selected_head" <<'PY'
import hashlib
import json
from pathlib import Path
import subprocess
import sys

output = Path(sys.argv[1])
head = sys.argv[2]
if subprocess.check_output(["git", "status", "--porcelain"], timeout=30).strip():
    raise SystemExit("release provenance requires a clean source checkout")
names = ("tl3", "tl3d", "tl3-deploy", "tl3-worker-supervisor")
(output / "SOURCE.json").write_text(json.dumps({"head": head, "binaries": {
    name: hashlib.sha256((output / name).read_bytes()).hexdigest() for name in names}}, sort_keys=True))
PY
mv -T "$staging" "$output"
trap - EXIT
cat "$output/SHA256SUMS"
printf 'Release artifacts: %s\n' "$output"
