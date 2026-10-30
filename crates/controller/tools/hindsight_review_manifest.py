"""Describe PR #113's pinned Hindsight, worker, store and crash checks for PrismaBuild."""
import json
from pathlib import Path

root = Path(__file__).resolve().parents[3]
env = {
    "PATH": "/home/rob/.cargo/bin:/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin",
    "RUSTUP_HOME": "/home/rob/.rustup", "CARGO_HOME": "/home/rob/.cargo",
    "KANI_HOME": "/home/rob/.kani", "CARGO_BUILD_JOBS": "2", "RUST_TEST_THREADS": "1",
    "OMP_NUM_THREADS": "1", "MKL_NUM_THREADS": "1", "OPENBLAS_NUM_THREADS": "1",
    "RAYON_NUM_THREADS": "1", "UV_CONCURRENT_DOWNLOADS": "2", "UV_CONCURRENT_BUILDS": "2",
    "TL3_TEST_UV": "/home/rob/.local/bin/uv",
}
common = {"cwd": str(root), "cpus": 2, "demand": {"mem_gb": 6}, "env": env,
          "priority": -10, "timeout_s": 3600}
checks = [
    ["cargo", "test", "-p", "tl3-controller", "-p", "tl3-store", "--tests", "--locked"],
    ["bash", "crates/controller/tools/verify.sh"],
    ["bash", "-c",
     "set -euo pipefail\n"
     "/home/rob/.local/bin/uv run --locked --project workers --python 3.14+gil python -m pytest -c workers/pyproject.toml workers/tests\n"
     "/home/rob/.local/bin/uv run --locked --project workers --python 3.14+gil python -m pytest -c harness/pyproject.toml harness/tests\n"
     "cargo build -p tl3-controller --example tl3-deploy --locked\n"
     'export TL3_DEPLOY_HELPER="$PWD/target/debug/examples/tl3-deploy"\n'
     "python3 -m unittest discover -s deploy/dl380g10 -p test_deploy.py\n"],
]
rows = [{**common, "tags": [host], "argv": check}
        for host in ("dl380g10", "sparky", "sparklina") for check in checks]
print(json.dumps(rows, indent=2))
