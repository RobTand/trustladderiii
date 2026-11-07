set -eu
fx_out=${FX_SCRATCH_CHECK_EVIDENCE:?fresh evidence prefix required}
test ! -e "$fx_out"
mkdir -p "$fx_out"
fx_clone=$(mktemp -d)
git clone --quiet --no-hardlinks --no-checkout "$PWD" "$fx_clone/source"
cd "$fx_clone/source"
git checkout --quiet --detach "${FX_SCRATCH_EXACT_HEAD:-HEAD}"
git rev-parse HEAD > "$fx_out/executed-head.txt"
git status --porcelain=v1 --untracked-files=all > "$fx_out/executed-clean.txt"
cat "$fx_out/executed-head.txt" "$fx_out/executed-clean.txt"
if [ -n "${FX_SCRATCH_EXACT_HEAD:-}" ]; then test "$(cat "$fx_out/executed-head.txt")" = "$FX_SCRATCH_EXACT_HEAD"; fi
test ! -s "$fx_out/executed-clean.txt"
export PATH="/home/rob/.cargo/bin:$PATH"
export PYTHONPATH="$PWD/harness/src"
export FX_E2E_EVIDENCE="$fx_out/e2e"
export FX_SCRATCH_EVIDENCE="$fx_out/helper"
export FX_HELPER_EVIDENCE="$fx_out/helper-pin"
set +e
fx_failed=0
check() { fx_name=$1; shift; "$@" > "$fx_out/$fx_name.log" 2>&1; fx_status=$?; printf '%s=%s\n' "$fx_name" "$fx_status" >> "$fx_out/verdict.txt"; tail -n 15 "$fx_out/$fx_name.log"; if [ "$fx_status" != 0 ]; then fx_failed=1; fi; }
if [ "${FX_SCRATCH_FINAL:-0}" = 1 ]; then
 check fmt /home/rob/.cargo/bin/cargo fmt --all -- --check
else
 check fmt /home/rob/.cargo/bin/cargo fmt --all
 git diff > "$fx_out/format.patch"
fi
check python /home/rob/venvs/pb-cpu/bin/python -m pytest -q --basetemp="$fx_clone/pytest" harness/tests/test_scratch_cleanup.py harness/tests/test_launch_failure.py harness/tests/test_runtime_launcher.py
for fx_test in ${FX_SCRATCH_TESTS:-generated_action_sigterm_unwinds_workspace generated_action_sigkill_reclaims_after_cancel_and_timeout terminal_scratch_guards_keep_ordinary_leftovers_and_reject_bad_external_evidence jobspec_and_native_access_environment_are_exact_and_exclude_caller_sentinels}; do
 check "$fx_test" /home/rob/.cargo/bin/cargo test -p tl3-controller --test run_backend_cli --locked "$fx_test" -- --exact --include-ignored --test-threads=1 --nocapture
done
if [ "${FX_SCRATCH_FINAL:-0}" = 1 ]; then
 check rust-cli /home/rob/.cargo/bin/cargo test -p tl3-controller --test run_backend_cli --locked -- --test-threads=1
 export FX_FIXTURE_PYTHON=/home/rob/venvs/pb-cpu/bin/python
 export FX_FIXTURE_EVIDENCE="$fx_out/stage-fixture"
 check fixture /home/rob/.cargo/bin/cargo test -p tl3-controller --example fx-stage-packet --locked -- --nocapture --include-ignored --test-threads=1
 check clippy /home/rob/.cargo/bin/cargo clippy -p tl3-controller --all-targets --locked -- -D warnings
 fx_lint=$(mktemp -d)
 check lint-venv /home/rob/venvs/pb-cpu/bin/python -m venv "$fx_lint"
 check lint-install "$fx_lint/bin/python" -m pip install --disable-pip-version-check ruff==0.14.14
 check ruff "$fx_lint/bin/ruff" check --isolated --select E9,F crates/controller/src/adapters/prismabuild/action.py harness/src/tl3_harness/runtime.py harness/tests/fx_action_e2e.py harness/tests/test_launch_failure.py harness/tests/test_scratch_cleanup.py crates/controller/tests/python/fx_native_scratch_helper.py crates/controller/tests/python/fx_scratch_helper.py tools/fx-scratch-requirements.py
 check mirror /home/rob/venvs/pb-cpu/bin/python tools/ci/mirror_lint.py
 check rust-builds /home/rob/venvs/pb-cpu/bin/python tools/bazel/rust_builds.py --check
 check requirements /home/rob/venvs/pb-cpu/bin/python tools/fx-scratch-requirements.py "$fx_out/requirements"
 git status --porcelain=v1 --untracked-files=all > "$fx_out/final-clean.txt"
 check final-clean test ! -s "$fx_out/final-clean.txt"
fi
git diff --name-only 5ab44937a3f2bb719ceca0afaa06d2c8cf41e97a HEAD | while IFS= read -r fx_source; do sha256sum "$fx_source"; done > "$fx_out/executed-source.sha256"
cat "$fx_out/verdict.txt"
exit "$fx_failed"
