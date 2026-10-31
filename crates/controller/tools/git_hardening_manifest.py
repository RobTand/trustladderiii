"""Print PR 129's red or final PrismaBuild evidence campaign."""
import argparse
import json
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("scope", choices=("red", "mutations", "hosts", "verify", "all"))
parser.add_argument("--checkout", type=Path, required=True)
args = parser.parse_args()
root = args.checkout.resolve()
if subprocess.check_output(["git", "status", "--porcelain"], cwd=root, text=True):
    raise SystemExit("Commit changes before submitting.")
if subprocess.run(["git", "symbolic-ref", "-q", "HEAD"], cwd=root,
                  stdout=subprocess.DEVNULL).returncode == 0:
    raise SystemExit("Use a clean detached worktree at the evidence commit.")
env = {
    "PATH": "/home/rob/.cargo/bin:/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin",
    "RUSTUP_HOME": "/home/rob/.rustup", "CARGO_HOME": "/home/rob/.cargo",
    "KANI_HOME": "/home/rob/.kani", "CARGO_BUILD_JOBS": "2", "RUST_TEST_THREADS": "2",
    "OMP_NUM_THREADS": "1", "MKL_NUM_THREADS": "1", "OPENBLAS_NUM_THREADS": "1",
    "RAYON_NUM_THREADS": "1", "TL3_TEST_UV": "/home/rob/.local/bin/uv",
    "UV_CONCURRENT_DOWNLOADS": "2", "UV_CONCURRENT_BUILDS": "2", "TMPDIR": "/home/rob/tmp",
}
# Keep embedded Postgres sockets short, and avoid the small default scratch dataset.
prepare = '''set -e
scratch=$(mktemp -d /home/rob/tmp/g.XXXXXX)
trap 'rm -rf -- "$scratch"' EXIT
export TMPDIR="$scratch"
'''
red = '''set +e
cargo test --locked -p tl3-controller --lib -- --nocapture; a=$?
cargo test --locked -p tl3-worktrees snapshot_retains_legacy_identity -- --nocapture; b=$?
cargo test --locked -p tl3-store --test postgres; c=$?
uv run --project harness --python /usr/bin/python3.14 --extra test python -m pytest harness/tests/test_git_environment.py -q; d=$?
uv run --locked --project workers --python /usr/bin/python3.14 python -m pytest workers/tests/test_pb_tests.py::test_staging_git_uses_closed_protocol_policy -q; e=$?
/usr/bin/python3.14 -m unittest discover -s crates/controller/tests/python -v; f=$?
exit $((a|b|c|d|e|f))'''
hosts = '''cargo test --locked -p tl3-worktrees
cargo test --locked -p tl3-controller --lib --test prismabuild --test publishing --test reproducing_rebase --test workers --test looks
cargo build --locked -p tl3-controller --example tl3-deploy
export TL3_DEPLOY_HELPER="$PWD/target/debug/examples/tl3-deploy"
if [ -x /usr/bin/python3.14 ]; then
    /usr/bin/python3.14 tools/ci/check.py python
else
    uv run --locked --project workers --python 3.14+gil python tools/ci/check.py python
fi
'''

def action(command, host=None):
    row = {"cwd": str(root), "argv": ["bash", "-c", prepare + command],
           "cpus": 4, "demand": {"mem_gb": 8}, "priority": -10,
           "timeout_s": 3600, "env": env}
    row.update({"tags": [host]} if host else {"anywhere": True})
    return row

rows = []
if args.scope == "red":
    rows.append(action(red, "dl380g10"))
if args.scope == "mutations":
    rows.append(action("cargo test --locked -p tl3-controller --lib hostile_ -- --nocapture", "dl380g10"))
if args.scope in ("hosts", "all"):
    rows.extend(action(hosts, host) for host in ("dl380g10", "sparky", "sparklina"))
if args.scope in ("verify", "all"):
    rows.append(action("bash crates/controller/tools/verify.sh"))
print(json.dumps(rows, indent=2))
