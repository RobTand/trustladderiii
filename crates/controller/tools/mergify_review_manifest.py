"""Describe PR 69's host-sensitive checks for PrismaBuild, without running them."""
import argparse
import json
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("suite", nargs="?", choices=("hosts", "workspace"), default="hosts")
args = parser.parse_args()

root = Path(__file__).resolve().parents[3]
if subprocess.check_output(["git", "status", "--porcelain"], cwd=root, text=True):
    raise SystemExit("Commit changes and use a clean detached worktree before submitting.")
if subprocess.run(["git", "symbolic-ref", "-q", "HEAD"], cwd=root, capture_output=True).returncode == 0:
    raise SystemExit("Use git worktree add --detach at the final commit before submitting.")
env = {
    "PATH": "/home/rob/.cargo/bin:/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin",
    "RUSTUP_HOME": "/home/rob/.rustup",
    "CARGO_HOME": "/home/rob/.cargo",
    "KANI_HOME": "/home/rob/.kani",
    "CARGO_BUILD_JOBS": "2",
    "RUST_TEST_THREADS": "2",
    "OMP_NUM_THREADS": "1",
    "MKL_NUM_THREADS": "1",
    "OPENBLAS_NUM_THREADS": "1",
}
common = {
    "cwd": str(root),
    "cpus": 2,
    "demand": {"mem_gb": 6},
    "priority": -10,
    "timeout_s": 3600,
    "env": env,
}
hosts = [{
    **common,
    "argv": ["cargo", "test", "-p", "tl3-controller", "-p", "tl3-github", "-p", "tl3-mergify", "-p", "tl3-store", "--all-targets", "--locked"],
    "tags": [host],
} for host in ["dl380g10", "sparky", "sparklina"]]
workspace = [{
    **common,
    "anywhere": True,
    "argv": ["bash", "crates/controller/tools/verify.sh"],
}]
print(json.dumps(hosts if args.suite == "hosts" else workspace, indent=2))
