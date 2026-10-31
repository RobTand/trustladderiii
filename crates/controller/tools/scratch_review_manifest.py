"""Print the stopped-scratch verification campaign for pbcampaign.py."""
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("suite", choices=("hosts", "verify"))
args = parser.parse_args()
root = Path(__file__).resolve().parents[3]
env = {
    "PATH": "/home/rob/.cargo/bin:/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin",
    "RUSTUP_HOME": "/home/rob/.rustup", "CARGO_HOME": "/home/rob/.cargo",
    "KANI_HOME": "/home/rob/.kani", "CARGO_BUILD_JOBS": "2",
    "RUST_TEST_THREADS": "2", "OMP_NUM_THREADS": "1", "RAYON_NUM_THREADS": "1",
    "MKL_NUM_THREADS": "1", "OPENBLAS_NUM_THREADS": "1",
    "UV_CONCURRENT_DOWNLOADS": "2", "UV_CONCURRENT_BUILDS": "2",
    "TL3_TEST_UV": "/home/rob/.local/bin/uv", "TMPDIR": "/home/rob/tmp",
    "TL3_SIM_FULL": "0", "TL3_SIM_SEED": "1450022",
}
common = {"cwd": str(root), "cpus": 2, "demand": {"mem_gb": 6}, "env": env,
          "timeout_s": 3600}
if args.suite == "hosts":
    command = """set -euo pipefail
cargo test -p tl3-worktrees --locked
cargo test -p tl3-controller --locked --test looks --test github --test publishing --test workers --test reproducing_rebase --test worktrees --test crash_safety
"""
    rows = [{**common, "tags": [host], "argv": ["bash", "-c", command]}
            for host in ("dl380g10", "sparky", "sparklina")]
else:
    rows = [{**common, "anywhere": True,
             "argv": ["bash", "crates/controller/tools/verify.sh"]}]
print(json.dumps(rows, indent=2))
