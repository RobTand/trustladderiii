#!/bin/bash
set +x
set -euo pipefail
IFS= read -r jit_config
[[ -n "$jit_config" ]] || exit 1
exec /home/runner/bin/Runner.Listener run --jitconfig "$jit_config"
