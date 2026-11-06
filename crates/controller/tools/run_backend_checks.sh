#!/usr/bin/env bash
# Execute only inside an admitted development action with shared storage.
set -uo pipefail
out=$1
mkdir -p "$out"
cargo fmt --all > "$out/fmt.log" 2>&1
fmt_status=$?
git diff --binary > "$out/fmt.patch"
cargo test -p tl3-controller --test run_backend_cli --locked -- --ignored --test-threads=4 > "$out/tests.log" 2>&1
test_status=$?
cargo clippy --workspace --all-targets --locked -- -D warnings > "$out/clippy.log" 2>&1
clippy_status=$?
printf 'fmt\t%s\nshared-mount-cli-tests\t%s\nclippy\t%s\n' "$fmt_status" "$test_status" "$clippy_status" > "$out/verdict.tsv"
cat "$out/verdict.tsv"
if [ "$fmt_status" -ne 0 ] || [ "$test_status" -ne 0 ] || [ "$clippy_status" -ne 0 ]; then
    exit 1
fi
