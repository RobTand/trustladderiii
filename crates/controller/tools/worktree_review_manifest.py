"""Print the worktree review campaign from a clean detached worktree.

Submit each suite with pbcampaign.py. Host tags qualify process and SIGKILL
behavior on every fleet host. The workspace suite is portable.
"""
import argparse
import json
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("suite", choices=("worktrees", "workspace"))
args = parser.parse_args()
root = Path(__file__).resolve().parents[3]
if subprocess.check_output(["git", "status", "--porcelain"], cwd=root, text=True):
    raise SystemExit("Commit changes and use a clean detached worktree before submitting.")
env = {
    "PATH": "/home/rob/.cargo/bin:/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin",
    "RUSTUP_HOME": "/home/rob/.rustup", "CARGO_HOME": "/home/rob/.cargo",
    "KANI_HOME": "/home/rob/.kani", "CARGO_BUILD_JOBS": "2",
    "RUST_TEST_THREADS": "2", "OMP_NUM_THREADS": "1",
    "MKL_NUM_THREADS": "1", "OPENBLAS_NUM_THREADS": "1",
}
rows = [{
    "cwd": str(root),
    "argv": ["cargo", "test", "-p", "tl3-worktrees", "-p", "tl3-controller", "--all-targets", "--locked"],
    "cpus": 2, "demand": {"mem_gb": 6}, "tags": [host], "env": env,
    "priority": -10,
} for host in ("dl380g10", "sparky", "sparklina")]
workspace = {
    "cwd": str(root), "argv": ["bash", "crates/controller/tools/verify.sh"],
    "cpus": 2, "demand": {"mem_gb": 6}, "anywhere": True, "env": env,
    "priority": -10,
}
print(json.dumps(rows if args.suite == "worktrees" else [workspace], indent=2))
