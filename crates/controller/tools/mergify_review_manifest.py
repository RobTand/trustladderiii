"""Describe PR 69's host-sensitive checks for PrismaBuild, without running them."""
import json
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parents[3]
if subprocess.check_output(["git", "status", "--porcelain"], cwd=root, text=True):
    raise SystemExit("Commit changes and use a clean detached worktree before submitting.")
env = {
    "PATH": "/home/rob/.cargo/bin:/usr/local/bin:/usr/bin:/bin",
    "RUSTUP_HOME": "/home/rob/.rustup",
    "CARGO_HOME": "/home/rob/.cargo",
    "KANI_HOME": "/home/rob/.kani",
    "CARGO_BUILD_JOBS": "2",
    "RUST_TEST_THREADS": "2",
    "OMP_NUM_THREADS": "1",
    "MKL_NUM_THREADS": "1",
    "OPENBLAS_NUM_THREADS": "1",
}
print(json.dumps([{
    "cwd": str(root),
    "argv": ["cargo", "test", "-p", "tl3-controller", "-p", "tl3-github", "-p", "tl3-mergify", "--all-targets", "--locked"],
    "tags": [host],
    "cpus": 2,
    "demand": {"mem_gb": 6},
    "priority": -10,
    "timeout_s": 3600,
    "env": env,
} for host in ["dl380g10", "sparky", "sparklina"]], indent=2))
