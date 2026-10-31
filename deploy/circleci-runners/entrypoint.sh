#!/bin/bash
set +x
set -euo pipefail
chown runner:runner /home/runner/.cache /work
# CircleCI removes the per-job directory during cleanup. The listener must not inherit it.
cd /
exec /usr/local/bin/circleci-runner machine --config /run/circleci/runner.yaml
