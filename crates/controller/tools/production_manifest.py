"""Describe #20's final checks for submission with pbcampaign.py."""
import argparse
import json
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--checkout", type=Path, default=Path(__file__).resolve().parents[3])
args = parser.parse_args()
root = args.checkout.resolve()
if subprocess.check_output(["git", "status", "--porcelain"], cwd=root, text=True):
    raise SystemExit("Commit before submitting.")
if subprocess.run(["git", "symbolic-ref", "-q", "HEAD"], cwd=root,
                  stdout=subprocess.DEVNULL).returncode == 0:
    raise SystemExit("Use a clean detached worktree at the final commit.")
env = {
    "PATH": "/home/rob/.cargo/bin:/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin",
    "RUSTUP_HOME": "/home/rob/.rustup", "CARGO_HOME": "/home/rob/.cargo",
    "KANI_HOME": "/home/rob/.kani", "CARGO_BUILD_JOBS": "2", "RUST_TEST_THREADS": "2",
    "OMP_NUM_THREADS": "1", "MKL_NUM_THREADS": "1", "OPENBLAS_NUM_THREADS": "1",
    "RAYON_NUM_THREADS": "1", "TL3_TEST_UV": "/home/rob/.local/bin/uv",
    "UV_CONCURRENT_DOWNLOADS": "2", "UV_CONCURRENT_BUILDS": "2",
}
rows = [{
    "cwd": str(root), "tags": [host], "cpus": 2, "demand": {"mem_gb": 6},
    "priority": -10, "timeout_s": 3600, "env": env,
    "argv": ["bash", "deploy/dl380g10/verify-production.sh"],
} for host in ["dl380g10", "sparky", "sparklina"]]
rows.append({
    "cwd": str(root), "anywhere": True, "cpus": 2, "demand": {"mem_gb": 6},
    "priority": -10, "timeout_s": 3600, "env": env,
    "argv": ["bash", "crates/controller/tools/verify.sh"],
})
print(json.dumps(rows, indent=2))
