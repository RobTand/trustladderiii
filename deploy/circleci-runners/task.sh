#!/bin/bash
set +x
set -euo pipefail
# The listener retains its root-only token. Only the task agent drops privileges.
exec setpriv --reuid=runner --regid=runner --init-groups --reset-env \
  env PATH=/home/runner/.cargo/bin:/home/runner/.local/bin:/usr/local/bin:/usr/bin:/bin \
  CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 OMP_NUM_THREADS=1 MKL_NUM_THREADS=1 \
  OPENBLAS_NUM_THREADS=1 RAYON_NUM_THREADS=1 TOKIO_WORKER_THREADS=1 "$@"
