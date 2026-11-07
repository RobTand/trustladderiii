set -eu
fx_out=${FX_SCRATCH_RED_EVIDENCE:?fresh evidence prefix required}
test ! -e "$fx_out"
mkdir -p "$fx_out"
fx_original=$(mktemp -d)
git clone --quiet --no-hardlinks --no-checkout "$PWD" "$fx_original/source"
git -C "$fx_original/source" checkout --quiet --detach 5dd6443393f8d1a6be6fd4ad7aaebc8e1af7f436
git -C "$fx_original/source" rev-parse HEAD > "$fx_out/action-head.txt"
git -C "$fx_original/source" status --porcelain=v1 --untracked-files=all > "$fx_out/action-clean.txt"
cat "$fx_out/action-head.txt" "$fx_out/action-clean.txt"
test ! -s "$fx_out/action-clean.txt"
sha256sum "$fx_original/source/crates/controller/src/adapters/prismabuild/action.py" "$fx_original/source/harness/src/tl3_harness/runtime.py" > "$fx_out/action-source.sha256"
cp "$fx_original/source/crates/controller/src/adapters/prismabuild/action.py" "$fx_out/original-action.py"
export FX_ACTION_SOURCE="$fx_original/source"
export FX_E2E_EVIDENCE="$fx_out/e2e"
set +e
/home/rob/.cargo/bin/cargo test -p tl3-controller --test run_backend_cli --locked generated_action_sigterm_unwinds_workspace -- --exact --include-ignored --test-threads=1 --nocapture > "$fx_out/cargo.log" 2>&1
fx_result=$?
cat "$fx_out/cargo.log"
printf '%s\n' "$fx_result" > "$fx_out/test-exit.txt"
exit "$fx_result"
