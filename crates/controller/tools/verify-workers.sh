#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
bash crates/controller/tools/verify.sh
