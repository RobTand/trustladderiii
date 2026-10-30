#!/bin/bash
set +x
set -euo pipefail
umask 077

slot=${1:?Usage: runner-slot.sh SLOT [SERVICE_PREFIX]}
prefix=${2:-tl3-ci-runner}
case "$prefix" in
    tl3-ci-runner)
        repository=RobTand/trustladderiii
        label=tl3-fleet
        runner_prefix=tl3-dl380g10
        [[ "$slot" =~ ^[1-6]$ ]] || exit 1
        ;;
    tl3-ci-runner-sandbox)
        repository=RobTand/tl3-sandbox
        label=tl3-sandbox-fleet
        runner_prefix=tl3-sandbox-dl380g10
        [[ "$slot" =~ ^[1-2]$ ]] || exit 1
        ;;
    *) echo 'Unknown runner service prefix' >&2; exit 1 ;;
esac
[[ ${CI_RUNNER_IMAGE:-} =~ ^sha256:[a-f0-9]{64}$ ]] || {
    echo 'CI_RUNNER_IMAGE must be the installed immutable image ID' >&2
    exit 1
}

container="$prefix-$slot"
runner="$runner_prefix-slot-$slot-$(date +%s)-${RANDOM}"
options=()
cpuset=${CI_RUNNER_CPUSET:-40-79}
[[ "$cpuset" =~ ^[0-9,-]+$ ]] || exit 1
options+=(--cpuset-cpus "$cpuset")

# Only this host process uses gh's login. Never trace or persist the JIT response.
jit_config=$(gh api --method POST \
    "repos/$repository/actions/runners/generate-jitconfig" \
    -f "name=$runner" -F runner_group_id=1 -f work_folder=_work \
    -f 'labels[]=self-hosted' -f 'labels[]=linux' -f 'labels[]=x64' \
    -f "labels[]=$label" --jq .encoded_jit_config 2>/dev/null) || {
    status=$?
    echo "JIT registration failed for slot $slot (gh exit $status); systemd will back off" >&2
    exit 1
}
[[ -n "$jit_config" && "$jit_config" != null ]] || exit 1
# systemd 259 retains the backoff counter across successful automatic restarts.
systemd-notify RESTART_RESET=1
echo "Starting $runner"

# stdin carries the one-job credential, outside Podman's command and environment.
printf '%s\n' "$jit_config" | podman run --rm --interactive \
    --name "$container" --pull=never --cpus=4 --memory=6g --memory-swap=6g \
    --pids-limit=1024 --network=private --http-proxy=false --sdnotify=ignore \
    --cap-drop=all --security-opt=no-new-privileges \
    --label io.trustladder.ci-slot="$slot" \
    "${options[@]}" "$CI_RUNNER_IMAGE"
