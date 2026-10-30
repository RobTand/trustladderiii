"""Describe PR #67's final checks from a clean detached worktree.

Submit this JSON with the published pbcampaign.py. This script executes no tests.
"""
import json
from pathlib import Path
import subprocess

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
        "PATH": "/home/rob/.cargo/bin:/usr/local/bin:/usr/bin:/bin",
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
             "--all-targets", "--locked"],
} for host in ("dl380g10", "sparky", "sparklina")]
rows.append({
    **common,
    "anywhere": True,
    "argv": ["bash", "crates/controller/tools/verify.sh"],
})
print(json.dumps(rows, indent=2))
