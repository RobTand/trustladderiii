#!/usr/bin/env bash
set -euo pipefail

run_wizard() {
  printf '\nStep 1/2: Create TL3\047s private GitHub App\n'
  printf 'The callback saves the app id, private key, webhook secret and client credentials.\n'
  printf 'Default destination: ~/.config/tl3/github-app.json on dl380g10.\n'
  local script_dir
  script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
  exec env PYTHONDONTWRITEBYTECODE=1 python3 -u "$script_dir/setup.py" "$@"
}

run_wizard "$@"
