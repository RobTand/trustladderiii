#!/bin/bash
set +x
set -euo pipefail
slot=${1:?slot required}
[[ $slot =~ ^[1-6]$ ]] || exit 2
config="$HOME/.config/circleci-fleet/runner-$slot.yaml"
[[ $(stat -c %a "$config") == 600 ]] || { echo 'Runner config must have mode 600' >&2; exit 2; }
args=(--rm --pull=never --name "circleci-fleet-$slot" --cpus=4 --memory=6g --memory-swap=6g
      --pids-limit=1024 --security-opt=no-new-privileges --cap-drop=all
      --cap-add=chown --cap-add=setuid --cap-add=setgid --cap-add=dac_override
      --volume "$config:/run/circleci/runner.yaml:ro"
      --volume "circleci-fleet-cache-$slot:/home/runner/.cache")
if [[ -n ${CIRCLECI_CPUSET:-} ]]; then args+=(--cpuset-cpus "$CIRCLECI_CPUSET"); fi
exec podman run "${args[@]}" "$CIRCLECI_IMAGE"
