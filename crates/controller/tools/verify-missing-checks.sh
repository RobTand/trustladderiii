#!/usr/bin/env bash
set -uo pipefail
cd "$(dirname "$0")/../../.."
cargo test --locked -p tl3-controller --test mergify --no-run || exit $?
result=0
for mode in path-filter wrong-app late-report rob-clear clock-running clock-missing ci-failed head-replaced; do
    cargo test --locked -p tl3-controller --test mergify -- "--missing-check-$mode" || result=1
done
exit "$result"
