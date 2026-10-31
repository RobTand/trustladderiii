#!/bin/bash
set +x
set -euo pipefail
# Install files only. Starting a slot is a separate capacity decision.
source_dir=$(cd "$(dirname "$0")" && pwd)
[[ $(podman info --format '{{.Host.Security.Rootless}}') == true ]]
build_dir=$(mktemp -d)
trap 'rm -rf "$build_dir"' EXIT
case $(uname -m) in
  x86_64)
    runner_sha=7bdbb338075142345af3fa9045a785bda5f83842a65b4af1a7d7f086ba767c46
    nextest_sha=b0959bc64232974224cf9fb22c915845398c0e8668fa7831eb5b43fb1eef781f
    nextest_asset=679066447
    arch=amd64 ;;
  aarch64)
    runner_sha=e714efa097b3dab8f8df763c3e2d9cb7e9f44638ab3f2450af781177fda738a2
    nextest_sha=9164568e9840c6509177086828ac7d3178c2d8feb0658b4d2bff62fa620e98c8
    nextest_asset=679066450
    arch=arm64 ;;
  *) echo 'Unsupported runner architecture' >&2; exit 2 ;;
esac
curl -fsSL "https://circleci-binary-releases.s3.amazonaws.com/circleci-runner/3.1.14-10146-3ce7306/circleci-runner_linux_$arch.tar.gz" -o "$build_dir/runner.tar.gz"
printf '%s  %s\n' "$runner_sha" "$build_dir/runner.tar.gz" | sha256sum -c -
gh api "repos/nextest-rs/nextest/releases/assets/$nextest_asset" -H 'Accept: application/octet-stream' > "$build_dir/nextest.tar.gz"
printf '%s  %s\n' "$nextest_sha" "$build_dir/nextest.tar.gz" | sha256sum -c -
tar -xzf "$build_dir/runner.tar.gz" -C "$build_dir" circleci-runner
tar -xzf "$build_dir/nextest.tar.gz" -C "$build_dir" cargo-nextest
cp "$source_dir/Containerfile" "$source_dir/task.sh" "$source_dir/entrypoint.sh" "$build_dir/"
podman build --cpus=2 --memory=6g --memory-swap=6g -t localhost/circleci-fleet:20261031 "$build_dir"
install -d -m 700 "$HOME/.config/circleci-fleet"
install -d "$HOME/.local/libexec/circleci-fleet" "$HOME/.config/systemd/user"
install -m 755 "$source_dir/slot.sh" "$HOME/.local/libexec/circleci-fleet/slot.sh"
install -m 644 "$source_dir/circleci-fleet@.service" "$HOME/.config/systemd/user/"
image=$(podman image inspect localhost/circleci-fleet:20261031 --format '{{.Id}}')
cpuset=
[[ $(hostname -s) != dl380g10 ]] || cpuset=40-79
printf 'CIRCLECI_IMAGE=%s\nCIRCLECI_CPUSET=%s\n' "$image" "$cpuset" > "$HOME/.config/circleci-fleet/runner.env"
systemctl --user daemon-reload
printf 'Installed %s; no slots started\n' "$image"
