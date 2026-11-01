#!/usr/bin/env bash
set -uo pipefail
cd "$(dirname "$0")/../../.."
cargo test --locked -p tl3-controller --test mergify --no-run || exit $?
result=0
cargo test --locked -p tl3-github --test rest required_status_checks || result=1
for mode in pass failure runner missing app status; do
    cargo test --locked -p tl3-controller --test mergify -- "--classic-head-$mode" || result=1
done
exit "$result"
