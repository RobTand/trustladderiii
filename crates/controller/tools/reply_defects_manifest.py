"""Generate the worker-reply regression campaign for PrismaBuild."""
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("suite", choices=("red", "hosts", "python", "workspace", "all"))
args = parser.parse_args()
root = Path(__file__).resolve().parents[3]
env = {
    "PATH": "/home/rob/.cargo/bin:/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin",
    "RUSTUP_HOME": "/home/rob/.rustup", "CARGO_HOME": "/home/rob/.cargo",
    "KANI_HOME": "/home/rob/.kani", "CARGO_BUILD_JOBS": "2",
    "RUST_TEST_THREADS": "2", "RAYON_NUM_THREADS": "1", "OMP_NUM_THREADS": "1",
    "MKL_NUM_THREADS": "1", "OPENBLAS_NUM_THREADS": "1",
    "UV_CONCURRENT_DOWNLOADS": "2", "UV_CONCURRENT_BUILDS": "2",
    "TL3_TEST_UV": "/home/rob/.local/bin/uv", "TMPDIR": "/home/rob/tmp",
}
common = {"cwd": str(root), "cpus": 2, "demand": {"mem_gb": 6}, "env": env}
def shell(command):
    return ["bash", "-ec", command]


rows = []
if args.suite in {"hosts", "all"}:
    # The workers target includes every maker scenario. Steering lives in github.
    rows.extend({**common, "tags": [host], "argv": shell(
        "cargo test --locked -p tl3-controller --lib --test workers "
        "--test publishing --test github --test reproducing_rebase"
    )} for host in ("dl380g10", "sparky", "sparklina"))
if args.suite in {"python", "all"}:
    # CI now runs four Python worker processes. This interpreter is on dl380g10.
    rows.append({**common, "cpus": 4, "demand": {"mem_gb": 8},
                 "tags": ["dl380g10"], "argv": shell("""
cargo build --locked -p tl3-controller --example tl3-deploy
export TL3_DEPLOY_HELPER="$PWD/target/debug/examples/tl3-deploy"
/usr/bin/python3.14 tools/ci/check.py python
""")})
if args.suite in {"workspace", "all"}:
    rows.append({**common, "anywhere": True,
                 "argv": shell("bash crates/controller/tools/verify.sh")})
if args.suite == "red":
    rows.append({**common, "anywhere": True, "argv": shell("""
status=0
cargo test --locked -p tl3-controller --lib defects -- --nocapture || status=1
/home/rob/.local/bin/uv run --locked --project workers --python 3.14+gil pytest -c workers/pyproject.toml workers/tests/test_planner.py workers/tests/test_maker.py harness/tests/test_node_id_vectors.py -q -k 'exact_node_ids or controller_selection' || status=1
cargo test --locked -p tl3-controller --test workers || status=1
exit "$status"
""")})
print(json.dumps(rows, indent=2))
