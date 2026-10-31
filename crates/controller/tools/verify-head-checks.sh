#!/usr/bin/env bash
set -uo pipefail
cd "$(dirname "$0")/../../.."
cargo test --locked -p tl3-controller --test mergify --no-run || exit $?
result=0
for mode in cancelled timed_out startup_failure failure permission restart otherhead optional label-present label-lag workflow-running forbidden-running forbidden-old forbidden-other grant-retry request-retry; do
    cargo test --locked -p tl3-controller --test mergify -- "--head-check-$mode" || result=1
done
exit "$result"
