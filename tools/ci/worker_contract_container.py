"""Replay the real maker/reviewer contract in a PB-owned CI runner image.

Run only inside an admitted PrismaBuild action, declaring the immutable image
with --container-image. The runner image and deployment files stay unchanged.
"""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[2]
image = sys.argv[1]
repetitions = int(sys.argv[2]) if len(sys.argv) > 2 else 1
with tempfile.TemporaryDirectory(prefix='tl3-contract-home-') as home:
    command = [
        'docker', 'run', '--rm', '--entrypoint', '/bin/bash',
        '--user', f'{os.getuid()}:{os.getgid()}',
        '--memory=6g', '--memory-swap=6g', '--pids-limit=1024',
        '--cap-drop=ALL', '--security-opt=no-new-privileges',
        '--mount', f'type=bind,src={root},dst={root}',
        '--mount', f'type=bind,src={home},dst=/tmp/contract-home',
        '--mount', 'type=bind,src=/home/rob/.local/bin/uv,dst=/usr/local/bin/uv,readonly',
        '--mount', 'type=bind,src=/home/rob/.local/share/uv/python/cpython-3.14.4-linux-aarch64-gnu,dst=/opt/python,readonly',
        '--workdir', str(root), '--env', 'HOME=/tmp/contract-home',
        '--env', 'RUSTUP_HOME=/home/runner/.rustup',
        '--env', 'CARGO_HOME=/tmp/contract-home/.cargo',
        '--env', 'CARGO_BUILD_JOBS=4', '--env', 'RUST_TEST_THREADS=4',
        '--env', 'OMP_NUM_THREADS=1', '--env', 'MKL_NUM_THREADS=1',
        '--env', 'OPENBLAS_NUM_THREADS=1', '--env', 'RAYON_NUM_THREADS=1',
        '--env', 'TOKIO_WORKER_THREADS=1', image, '-ec',
        'export PATH=/opt/python/bin:/home/runner/.cargo/bin:/usr/local/bin:/usr/bin:/bin\n'
        'cargo test --locked -p tl3-controller --test workers -- --real-worker-contracts\n'
        'cargo test --locked -p tl3-controller --test github -- --real-worker-contracts\n'
        'cargo test --locked -p tl3-controller --test workers --no-run\n'
        f'for iteration in $(seq 1 {repetitions}); do\n'
        '  echo "Real maker/reviewer replay $iteration"\n'
        '  cargo test --locked -p tl3-controller --test workers -- --maker-reproducing-rebase\n'
        'done',
    ]
    subprocess.run(command, check=True)
