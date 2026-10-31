#!/bin/bash
set +x
set -euo pipefail
chown -R runner:runner /home/runner/.cache
chown runner:runner /work
# CircleCI removes the per-job directory during cleanup. The listener must not inherit it.
cd /
exec /usr/local/bin/circleci-runner machine --config /run/circleci/runner.yaml
