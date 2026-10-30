"""Describe ticket 14's PrismaBuild verification from a clean checkout."""
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
    "RUST_TEST_THREADS": "2", "OMP_NUM_THREADS": "1", "RAYON_NUM_THREADS": "1",
    "MKL_NUM_THREADS": "1", "OPENBLAS_NUM_THREADS": "1",
}
common = {"cwd": str(root), "cpus": 2, "demand": {"mem_gb": 6}, "env": env, "priority": -10}
if args.suite == "hosts":
    rows = [{**common, "tags": [host], "argv": ["cargo", "test", "-p", "tl3-controller", "-p", "tl3-store", "-p", "tl3-github", "--tests", "--locked"]}
                for host in ("dl380g10", "sparky", "sparklina")]
else:
    rows = [{**common, "anywhere": True, "argv": ["bash", "crates/controller/tools/verify.sh"]}]
print(json.dumps(rows, indent=2))
