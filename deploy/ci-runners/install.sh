#!/bin/bash
set +x
set -euo pipefail
umask 077

slots=${1:-1}
profile=${2:-tl3}
case "$profile:$slots" in
    tl3:0|tl3:1|tl3:6) prefix=tl3-ci-runner ;;
    sandbox:0|sandbox:1|sandbox:2) prefix=tl3-ci-runner-sandbox ;;
    *) echo 'Usage: install.sh [0|1|6] [tl3] or install.sh [0|1|2] sandbox' >&2; exit 1 ;;
esac
[[ $(uname -m) == x86_64 && $(id -u) != 0 ]] || {
    echo 'Install as the rootless runner owner on x86_64' >&2
    exit 1
}
for tool in podman gh systemctl; do
    command -v "$tool" >/dev/null
done
[[ $(podman info --format '{{.Host.Security.Rootless}}') == true ]] || exit 1
if [[ -n ${CI_RUNNER_CPUSET:-} && ! "$CI_RUNNER_CPUSET" =~ ^[0-9,-]+$ ]]; then
    echo 'CI_RUNNER_CPUSET must be a CPU list' >&2
    exit 1
fi

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
podman build --cpu-period=100000 --cpu-quota=200000 --memory=6g \
    --tag localhost/tl3-ci-runner:phase1 "$here"
image=$(podman image inspect localhost/tl3-ci-runner:phase1 --format '{{.Id}}')
image="sha256:${image#sha256:}"
[[ "$image" =~ ^sha256:[a-f0-9]{64}$ ]] || exit 1

install -d -m 700 "$HOME/.local/lib/tl3-ci-runners" "$HOME/.config/tl3-ci-runners"
install -d -m 755 "$HOME/.config/systemd/user"
install -C -m 700 "$here/runner-slot.sh" "$HOME/.local/lib/tl3-ci-runners/runner-slot.sh"
install -C -m 644 "$here/tl3-ci-runner@.service" "$HOME/.config/systemd/user/$prefix@.service"
config="$HOME/.config/tl3-ci-runners/$prefix.env"
# Migrate the original TL3 environment without losing its CPU placement.
if [[ "$profile" == tl3 && ! -f "$config" && -f "$HOME/.config/tl3-ci-runners/runner.env" ]]; then
    install -m 600 "$HOME/.config/tl3-ci-runners/runner.env" "$config"
fi
# Preserve an operator's CPU placement on an ordinary rerun.
if [[ ! -f "$config" || ${CI_RUNNER_CPUSET+x} ]]; then
    printf 'CI_RUNNER_IMAGE=%s\nCI_RUNNER_CPUSET=%s\n' "$image" "${CI_RUNNER_CPUSET:-}" \
        > "$config"
else
    sed -i "s/^CI_RUNNER_IMAGE=.*/CI_RUNNER_IMAGE=$image/" "$config"
fi
systemctl --user daemon-reload
for ((slot=1; slot<=slots; slot++)); do
    systemctl --user enable --now "$prefix@$slot.service"
done
printf 'Installed image %s; requested %s active slots\n' "$image" "$slots"
