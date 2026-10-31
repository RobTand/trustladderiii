"""Generate ticket 145's PrismaBuild verification actions."""
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("suite", choices=("red", "hosts", "python", "workspace", "all", "fix-round"))
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
common = {"cwd": str(root), "cpus": 2, "demand": {"mem_gb": 6}, "env": env, "priority": -10}
rows = []
if args.suite == "red":
    rows.append({**common, "anywhere": True,
                 "argv": ["bash", "crates/controller/tools/verify-scope.sh"]})
if args.suite in {"hosts", "all"}:
    rows.extend({**common, "tags": [host], "argv": ["bash", "-ec",
        "cargo test --locked -p tl3-worktrees\n"
        "cargo test --locked -p tl3-controller --lib --test scope --test writer_quote"]}
        for host in ("dl380g10", "sparky", "sparklina"))
if args.suite in {"python", "all"}:
    rows.append({**common, "tags": ["dl380g10"], "argv": ["bash", "-ec", """
cargo build --locked -p tl3-controller --example tl3-deploy
export TL3_DEPLOY_HELPER="$PWD/target/debug/examples/tl3-deploy"
"$TL3_TEST_UV" run --locked --project workers --python /usr/bin/python3.14 pytest -q workers/tests/test_reviewer.py workers/tests/test_writer.py --basetemp "$PWD/scope-pytest"
/usr/bin/python3.14 -m unittest discover -s deploy/dl380g10 -p 'test_*.py' -v
"""]})
if args.suite in {"workspace", "all"}:
    rows.append({**common, "anywhere": True,
                 "argv": ["bash", "crates/controller/tools/verify.sh"]})
if args.suite == "fix-round":
    for host in ("dl380g10", "sparky"):
        python = "/usr/bin/python3.14" if host == "dl380g10" else "3.14+gil"
        rows.append({**common, "tags": [host], "argv": ["bash", "-ec", f"""
cargo test --locked -p tl3-worktrees
cargo test --locked -p tl3-controller --lib --test scope --test publishing --test writer_quote
cargo build --locked -p tl3-controller --example tl3-deploy
export TL3_DEPLOY_HELPER="$PWD/target/debug/examples/tl3-deploy"
"$TL3_TEST_UV" run --locked --project workers --python {python} pytest -q workers/tests/test_reviewer.py workers/tests/test_writer.py --basetemp "$PWD/scope-pytest"
"""]})
print(json.dumps(rows, indent=2))
