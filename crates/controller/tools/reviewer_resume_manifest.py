"""Describe ticket 88 verification for pbcampaign.py."""
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("suite", choices=("red", "hosts", "workspace"))
args = parser.parse_args()
root = Path(__file__).resolve().parents[3]
env = {
    "PATH": "/home/rob/.cargo/bin:/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin",
    "RUSTUP_HOME": "/home/rob/.rustup", "CARGO_HOME": "/home/rob/.cargo",
    "KANI_HOME": "/home/rob/.kani", "CARGO_BUILD_JOBS": "2",
    "RUST_TEST_THREADS": "2", "OMP_NUM_THREADS": "1",
    "MKL_NUM_THREADS": "1", "OPENBLAS_NUM_THREADS": "1",
    "UV_CONCURRENT_DOWNLOADS": "2", "UV_CONCURRENT_BUILDS": "2",
    "TL3_TEST_UV": "/home/rob/.local/bin/uv",
}
command = (
    "/home/rob/.local/bin/uv run --locked --project workers --python 3.14+gil "
    "pytest -c workers/pyproject.toml workers/tests -q; "
    "cargo test --locked -p tl3-controller --test publishing --test workers"
)
rows = [{
    "cwd": str(root), "argv": ["bash", "-ec", command],
    "cpus": 2, "demand": {"mem_gb": 6}, "tags": [host], "env": env,
} for host in ("dl380g10", "sparky", "sparklina")]
if args.suite == "red":
    rows = [{
        "cwd": str(root), "argv": ["bash", "-c", """status=0
for mode in changes round-one-rebase; do
    cargo test --locked -p tl3-controller --test publishing -- "$mode" || status=1
done
/home/rob/.local/bin/uv run --locked --project workers --python 3.14+gil pytest -c workers/pyproject.toml workers/tests/test_reviewer.py -q -k stops_before_context_exhaustion || status=1
exit "$status"
"""],
        "cpus": 2, "demand": {"mem_gb": 6}, "anywhere": True, "env": env,
    }]
if args.suite == "workspace":
    rows = [{
        "cwd": str(root), "argv": ["bash", "crates/controller/tools/verify.sh"],
        "cpus": 2, "demand": {"mem_gb": 6}, "anywhere": True, "env": env,
    }]
print(json.dumps(rows, indent=2))
