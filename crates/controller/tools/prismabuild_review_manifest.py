"""Describe PR #67's final checks from a clean detached worktree.

Submit this JSON with the published pbcampaign.py. This script executes no tests.
"""
import argparse
import json
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("suite", choices=("hosts", "workspace"))
args = parser.parse_args()
root = Path(__file__).resolve().parents[3]
if subprocess.check_output(["git", "status", "--porcelain"], cwd=root, text=True):
    raise SystemExit("commit changes before generating verification actions")
if subprocess.run(["git", "symbolic-ref", "-q", "HEAD"], cwd=root,
                  stdout=subprocess.DEVNULL, check=False).returncode == 0:
    raise SystemExit("use git worktree add --detach at the final commit")

common = {
    "cwd": str(root),
    "cpus": 2,
    "demand": {"mem_gb": 6},
    "priority": -10,
    "env": {
        "PATH": "/home/rob/.cargo/bin:/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin",
        "RUSTUP_HOME": "/home/rob/.rustup",
        "CARGO_HOME": "/home/rob/.cargo",
        "KANI_HOME": "/home/rob/.kani",
        "CARGO_BUILD_JOBS": "2",
        "RUST_TEST_THREADS": "2",
        "OMP_NUM_THREADS": "1",
        "MKL_NUM_THREADS": "1",
        "OPENBLAS_NUM_THREADS": "1",
    },
}
rows = [{
    **common,
    "tags": [host],
    "argv": ["cargo", "test", "-p", "tl3-controller", "-p", "tl3-prismabuild",
             "-p", "tl3-store", "--all-targets", "--locked"],
} for host in ("dl380g10", "sparky", "sparklina")]
workspace = {
    **common,
    "anywhere": True,
    "argv": ["bash", "crates/controller/tools/verify.sh"],
}
print(json.dumps(rows if args.suite == "hosts" else [workspace], indent=2))
