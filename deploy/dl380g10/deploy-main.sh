#!/usr/bin/env bash
# Canonical fresh-init caller. The installed external entry point delegates here.
set -euo pipefail
# Discard inherited Git identity/config overrides before any qualification.
for key in ${!GIT_@}; do unset "$key"; done
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0
sha=${1:?one full exact qualified release SHA is required}
[[ $# = 1 && "$sha" =~ ^[0-9a-f]{40}$ ]] || { echo 'full exact SHA required' >&2; exit 1; }
tree=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)
python3 -I - "$tree" "$sha" <<'PY'
import os
from pathlib import Path
import stat
import subprocess
import sys
tree = Path(sys.argv[1])
for path in (tree, tree / '.git'):
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o022:
        raise SystemExit('exact source must be an owned independent clone')
for path in (tree, *tree.parents):
    if path.is_symlink():
        raise SystemExit('symlink source boundary refused')
def git(*args):
    return subprocess.check_output(['git', *args], cwd=tree, stderr=subprocess.DEVNULL, timeout=30).strip()
if (git('rev-parse', '--show-toplevel').decode() != str(tree)
        or git('rev-parse', 'HEAD').decode() != sys.argv[2]
        or git('status', '--porcelain=v1', '--untracked-files=all', '--ignored')
        or git('rev-parse', '--is-shallow-repository') != b'false'):
    raise SystemExit('exact source identity, cleanliness or complete history refused')
git('rev-list', '--objects', '--missing=error', sys.argv[2])
PY
notes=/home/rob/tmp/tl3-build
manifest=${TL3_FRESH_STATE_MANIFEST:?set the reviewed private exact-state manifest}
backup=${TL3_COLD_BACKUP:?set a new private cold-backup destination}
lane=${TL3_DEPLOY_LANE:-release}
case "$lane" in dev|release) ;; *) echo 'unknown qualification lane'; exit 1;; esac
cd "$tree"
timeout --kill-after=5s 610s python3 -I deploy/dl380g10/deploy.py qualify-fresh --head "$sha" --lane "$lane"
if [ "${TL3_DEPLOY_DRY_RUN:-0}" = 1 ]; then
  echo "exact-head qualification passed; dry run ends before build or service changes"
  exit 0
fi
# Native admission owns build and fresh-init. Explicit reattach reuses a dispatch;
# a lost reply never creates a replacement job.
replay=()
if [[ -n ${TL3_FLUX_DEPLOY_REATTACH:-} ]]; then
  replay=(--reattach "$TL3_FLUX_DEPLOY_REATTACH")
fi
if [[ -n ${TL3_FLUX_DEPLOY_BUILD_DISPATCH:-} ]]; then
  replay+=(--build-dispatch "$TL3_FLUX_DEPLOY_BUILD_DISPATCH")
fi
python3 -I "$tree/deploy/dl380g10/flux-deploy.py" admit "$sha" "$lane" "$manifest" "$backup" "${replay[@]}"
printf '%s\n' "$sha" > "$notes/deployed-sha"
printf -- '- %s DEPLOYED %s fresh-init; exact qualification, verified cold backup, current schema and bounded stability passed.\n' \
  "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$sha" >> "$notes/state.md"
