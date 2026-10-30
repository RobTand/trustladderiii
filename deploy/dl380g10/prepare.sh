#!/usr/bin/env bash
set -euo pipefail
deployment_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
python3 "$deployment_dir/deploy.py" install "$@"
operations_first=$("$HOME/.local/share/tl3/bin/tl3-deploy" operations "$HOME/.config/tl3/tl3.toml")
printf '%s\n' "$operations_first"
operations_second=$("$HOME/.local/share/tl3/bin/tl3-deploy" operations "$HOME/.config/tl3/tl3.toml")
printf '%s\n' "$operations_second"
if [[ "$operations_first" != "$operations_second" ]]; then
  printf 'Operations issue identity changed on rerun\n' >&2
  exit 1
fi
python3 "$deployment_dir/deploy.py" install "$@"
python3 "$deployment_dir/deploy.py" start
python3 "$deployment_dir/verify-live.py"
