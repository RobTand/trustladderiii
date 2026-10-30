"""Describe ticket 14's PrismaBuild verification from a clean checkout."""
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("suite", choices=("hosts", "workers", "workspace", "steering", "writer", "references"))
args = parser.parse_args()
root = Path(__file__).resolve().parents[3]
env = {
    "PATH": "/home/rob/.cargo/bin:/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin",
    "RUSTUP_HOME": "/home/rob/.rustup", "CARGO_HOME": "/home/rob/.cargo",
    "KANI_HOME": "/home/rob/.kani", "CARGO_BUILD_JOBS": "2",
    "RUST_TEST_THREADS": "2", "OMP_NUM_THREADS": "1", "RAYON_NUM_THREADS": "1",
    "MKL_NUM_THREADS": "1", "OPENBLAS_NUM_THREADS": "1",
    "UV_CONCURRENT_DOWNLOADS": "2", "UV_CONCURRENT_BUILDS": "2",
    "TL3_TEST_UV": "/home/rob/.local/bin/uv",
}
common = {"cwd": str(root), "cpus": 2, "demand": {"mem_gb": 6}, "env": env, "priority": -10}
if args.suite == "references":
    rows = [{**common, "tags": [host], "argv": ["bash", "-c", "cargo test -p tl3-controller --test writer_references --test writer_quote --test github --locked && cargo test -p tl3-github --test messages --locked"]}
            for host in ("dl380g10", "sparky", "sparklina")]
elif args.suite == "steering":
    rows = [{**common, "tags": [host], "argv": ["cargo", "test", "-p", "tl3-controller", "--test", "github", "--locked"]}
            for host in ("dl380g10", "sparky", "sparklina")]
elif args.suite == "writer":
    rows = [{**common, "anywhere": True, "argv": ["bash", "-c", "cd workers && /home/rob/.local/bin/uv run --locked --no-editable --python 3.14+gil pytest -q tests/test_writer.py"]}]
elif args.suite == "hosts":
    rows = [{**common, "tags": [host], "argv": ["cargo", "test", "-p", "tl3-controller", "-p", "tl3-store", "-p", "tl3-github", "-p", "tl3-mergify", "--tests", "--locked"]}
                for host in ("dl380g10", "sparky", "sparklina")]
elif args.suite == "workers":
    rows = [{**common, "tags": [host], "argv": ["bash", "-c", "cd workers && /home/rob/.local/bin/uv run --locked --no-editable --python 3.14+gil pytest -q"]}
            for host in ("dl380g10", "sparky", "sparklina")]
else:
    rows = [{**common, "anywhere": True, "argv": ["bash", "crates/controller/tools/verify.sh"]}]
print(json.dumps(rows, indent=2))
