#!/usr/bin/env bash
# Canonical fresh-init caller. The installed external entry point delegates here.
set -euo pipefail
repo=/home/rob/.t3/projects/trustladderiii
tools=/mnt/shared/prismabuild-fleet/repo/tools
notes=/home/rob/tmp/tl3-build
manifest=${TL3_FRESH_STATE_MANIFEST:?set the reviewed private exact-state manifest}
backup=${TL3_COLD_BACKUP:?set a new private cold-backup destination}
lane=${TL3_DEPLOY_LANE:-release}
case "$lane" in dev|release) ;; *) echo 'unknown qualification lane'; exit 1;; esac
git -C "$repo" fetch -q origin
sha=$(git -C "$repo" rev-parse --verify "${1:-origin/main}^{commit}")
tree=$notes/deploy-source-$sha
release=$notes/release-$sha
if [ ! -d "$tree" ]; then git -C "$repo" worktree add -q --detach "$tree" "$sha"; fi
[ "$(git -C "$tree" rev-parse HEAD)" = "$sha" ]
[ -z "$(git -C "$tree" status --porcelain)" ]
cd "$tree"
python3 -I deploy/dl380g10/deploy.py qualify-fresh --head "$sha" --lane "$lane"
if [ "${TL3_DEPLOY_DRY_RUN:-0}" = 1 ]; then
  echo "exact-head qualification passed; dry run ends before build or service changes"
  exit 0
fi
# This build exports to dl380g10's local artifact directory. Installation needs
# that user's local units/socket/home. Both have an actual host dependency.
if [ ! -f "$release/SOURCE.json" ]; then
  python3 "$tools/pbrun.py" --cwd "$tree" --tag dl380g10 --cpus 2 --demand mem_gb=8 \
    --timeout-s 1800 --wait-s 1800 \
    --env PATH=/home/rob/.cargo/bin:/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin \
    --env CARGO_BUILD_JOBS=2 --env OMP_NUM_THREADS=1 --env MKL_NUM_THREADS=1 \
    --env OPENBLAS_NUM_THREADS=1 --env RAYON_NUM_THREADS=1 \
    -- python3 -I deploy/dl380g10/exact-source.py "$sha" bash deploy/dl380g10/build-release.sh "$release" "$sha"
fi
python3 "$tools/pbrun.py" --cwd "$tree" --tag dl380g10 --cpus 1 --demand mem_gb=2 \
  --timeout-s 1800 --wait-s 1800 \
  --env PATH=/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin --env XDG_RUNTIME_DIR=/run/user/1000 \
  --env TL3_DEPLOY_NONCE="$(date -u +%Y%m%dT%H%M%S%N)" \
  --env OMP_NUM_THREADS=1 --env MKL_NUM_THREADS=1 --env OPENBLAS_NUM_THREADS=1 \
  -- python3 -I deploy/dl380g10/exact-source.py "$sha" python3 -I deploy/dl380g10/deploy.py fresh-init --head "$sha" --lane "$lane" \
      --binaries "$release" --state-manifest "$manifest" --backup "$backup"
# pbrun's synchronous verdict verifies the terminal outcome and CAS receipt.
# The selected helper never starts a previous binary after a failure.
printf '%s\n' "$sha" > "$notes/deployed-sha"
printf -- '- %s DEPLOYED %s fresh-init; exact qualification, verified cold backup, current schema and bounded stability passed.\n' \
  "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$sha" >> "$notes/state.md"
