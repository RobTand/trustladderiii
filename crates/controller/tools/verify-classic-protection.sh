#!/usr/bin/env bash
set -uo pipefail
cd "$(dirname "$0")/../../.."
cargo test --locked -p tl3-controller --test mergify --no-run || exit $?
result=0
cargo test --locked -p tl3-github --test rest required_status_checks || result=1
for mode in pass failure stale-failure runner runner-code missing app status action_required stale restricted-branch403 restricted-branch404 restricted-rules403 restricted-rules404 restricted-status403 restricted-status404; do
    cargo test --locked -p tl3-controller --test mergify -- "--classic-head-$mode" || result=1
done
exit "$result"
