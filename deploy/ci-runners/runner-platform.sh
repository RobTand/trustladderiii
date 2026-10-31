#!/bin/bash
# Shared by the installer and installed slot script. No credentials here.
case "$(uname -m)" in
    x86_64) image_arch=amd64; runner_arch=x64 ;;
    aarch64) image_arch=arm64; runner_arch=ARM64 ;;
    *) echo 'Unsupported runner architecture' >&2; exit 1 ;;
esac
runner_host=$(hostname -s)
[[ "$runner_host" =~ ^[a-zA-Z0-9][a-zA-Z0-9-]*$ ]] || exit 1
default_cpuset=
[[ "$runner_host" != dl380g10 ]] || default_cpuset=40-79
