#!/bin/bash
set +x
set -euo pipefail
chown runner:runner /home/runner/.cache /work
exec /usr/local/bin/circleci-runner machine --config /run/circleci/runner.yaml
