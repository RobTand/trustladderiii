"""Print PR #60's verification campaign from a clean detached worktree.

Every row executes on PrismaBuild. Host tags are required for process tests.
"""
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("suite", choices=("workers", "workspace"))
args = parser.parse_args()
root = Path(__file__).resolve().parents[3]
env = {
    "PATH": "/home/rob/.cargo/bin:/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin",
    "RUSTUP_HOME": "/home/rob/.rustup", "CARGO_HOME": "/home/rob/.cargo",
    "KANI_HOME": "/home/rob/.kani", "CARGO_BUILD_JOBS": "2",
    "RUST_TEST_THREADS": "2", "OMP_NUM_THREADS": "1",
    "MKL_NUM_THREADS": "1", "OPENBLAS_NUM_THREADS": "1",
}
rows = [{
    "cwd": str(root), "argv": ["cargo", "test", "-p", "tl3-controller", "--test", "workers", "--locked"],
    "cpus": 2, "demand": {"mem_gb": 6}, "tags": [host], "env": env,
} for host in ("dl380g10", "sparky", "sparklina")]
workspace = {
    "cwd": str(root), "argv": ["bash", "crates/controller/tools/verify-workers.sh"],
    "cpus": 2, "demand": {"mem_gb": 6}, "anywhere": True, "env": env,
}
print(json.dumps(rows if args.suite == "workers" else [workspace], indent=2))
