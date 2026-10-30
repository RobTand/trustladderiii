"""Print the worker verification campaign from a clean worktree.

Every row executes on PrismaBuild. Host tags cover worker, controller and store process tests.
"""
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("suite", choices=("workers", "maker", "workspace"))
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
    "cwd": str(root), "argv": ["cargo", "test", "-p", "tl3-controller", "--test", "workers", "--test", "github", "--locked"] if args.suite == "maker" else ["cargo", "test", "-p", "tl3-controller", "-p", "tl3-store", "--tests", "--locked"],
    "cpus": 2, "demand": {"mem_gb": 6}, "tags": [host], "env": env, "priority": -10,
} for host in ("dl380g10", "sparky", "sparklina")]
workspace = {
    "cwd": str(root), "argv": ["bash", "crates/controller/tools/verify-workers.sh"],
    "cpus": 2, "demand": {"mem_gb": 6}, "anywhere": True, "env": env, "priority": -10,
}
if args.suite == "maker":
    rows.append({**workspace, "argv": ["bash", "crates/controller/tools/verify-maker.sh"]})
print(json.dumps([workspace] if args.suite == "workspace" else rows, indent=2))
