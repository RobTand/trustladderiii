"""Describe ticket 13 verification. Submit the manifest with pbcampaign.py."""
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("suite", choices=("hosts", "workspace"))
args = parser.parse_args()
root = Path(__file__).resolve().parents[3]
env = {
    "PATH": "/home/rob/.cargo/bin:/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin",
    "RUSTUP_HOME": "/home/rob/.rustup", "CARGO_HOME": "/home/rob/.cargo",
    "KANI_HOME": "/home/rob/.kani", "CARGO_BUILD_JOBS": "2",
    "RUST_TEST_THREADS": "2", "OMP_NUM_THREADS": "1",
    "MKL_NUM_THREADS": "1", "OPENBLAS_NUM_THREADS": "1",
    "UV_CONCURRENT_DOWNLOADS": "2", "UV_CONCURRENT_BUILDS": "2",
}
rows = [{
    "cwd": str(root), "argv": ["bash", "crates/controller/tools/verify-publishing.sh"], "cpus": 2, "demand": {"mem_gb": 6},
    "tags": [host], "env": env, "priority": -10,
} for host in ("dl380g10", "sparky", "sparklina")]
if args.suite == "workspace":
    rows = [{
        "cwd": str(root), "argv": ["bash", "crates/controller/tools/verify.sh"],
        "cpus": 2, "demand": {"mem_gb": 6}, "anywhere": True,
        "env": env, "priority": -10,
    }]
print(json.dumps(rows, indent=2))
