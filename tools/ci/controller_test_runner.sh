#!/usr/bin/env bash
set -euo pipefail

controller=false
if [[ ${1:-} == --controller ]]; then
    controller=true
    shift
elif [[ ${CARGO_PKG_NAME:-} == tl3-controller && $(basename "$(dirname "$1")") == deps ]]; then
    controller=true
fi
binary=$1
shift
if "$controller"; then
    if [[ -z ${TL3_BINARY+x} ]]; then
        if [[ ${CARGO_PKG_NAME:-} == tl3-controller ]]; then
            TL3_BINARY=$(dirname "$binary")/../tl3
        else
            TL3_BINARY=$(dirname "$binary")/tl3
        fi
    fi
    if [[ ! -f "$TL3_BINARY" || ! -x "$TL3_BINARY" ]]; then
        echo "controller test setup: TL3_BINARY '$TL3_BINARY' is missing or not executable; build and supply the tl3 check-reply binary before running scenarios" >&2
        exit 1
    fi
    export TL3_BINARY
fi
exec "$binary" "$@"
