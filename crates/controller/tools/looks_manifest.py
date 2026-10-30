"""Print ticket 15's PrismaBuild verification campaign from a clean commit."""
import argparse
import json
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--checkout", type=Path, default=Path(__file__).resolve().parents[3])
parser.add_argument("--red", action="store_true", help="run the focused review regressions on dl380g10")
parser.add_argument("--merge-regressions", action="store_true", help="run the maker wait and snapshot ownership regressions")
args = parser.parse_args()
root = args.checkout.resolve()
if subprocess.check_output(["git", "status", "--porcelain"], cwd=root, text=True):
    raise SystemExit("Commit changes before submitting.")
if subprocess.run(["git", "symbolic-ref", "-q", "HEAD"], cwd=root,
                  stdout=subprocess.DEVNULL).returncode == 0:
    raise SystemExit("Use git worktree add --detach at the final commit.")
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
    "argv": ["cargo", "test", "-p", "tl3-controller", "-p", "tl3-github", "-p", "tl3-store", "--all-targets", "--locked"],
} for host in ["dl380g10", "sparky", "sparklina"]]
rows.append({
    "cwd": str(root), "anywhere": True, "cpus": 2, "demand": {"mem_gb": 6},
    "priority": -10, "timeout_s": 3600, "env": env,
    "argv": ["bash", "crates/controller/tools/verify.sh"],
})
# Worker protocol tests create subprocesses too, so they have the same placements.
rows.extend({
    "cwd": str(root), "tags": [host], "cpus": 2, "demand": {"mem_gb": 6},
    "priority": -10, "timeout_s": 3600, "env": env,
    "argv": ["/home/rob/.local/bin/uv", "run", "--locked", "--project", "workers", "--python", "3.14+gil", "python", "-m", "pytest", "workers/tests"],
} for host in ["dl380g10", "sparky", "sparklina"])
if args.red:
    rows = [{
        "cwd": str(root), "tags": ["dl380g10"], "cpus": 2, "demand": {"mem_gb": 6},
        "priority": -10, "timeout_s": 3600, "env": env,
        "argv": ["bash", "-c", "cargo test -p tl3-controller --locked --test looks -- --r3; a=$?; cargo test -p tl3-controller --locked --test workers -- --r3; b=$?; cargo test -p tl3-controller --locked --test prismabuild -- --r3; c=$?; cargo test -p tl3-store --locked --test postgres -- --r3; d=$?; exit $((a|b|c|d))"],
    }]
if args.merge_regressions:
    rows = [{
        "cwd": str(root), "tags": ["dl380g10"], "cpus": 2, "demand": {"mem_gb": 6},
        "priority": -10, "timeout_s": 3600, "env": env,
        "argv": ["bash", "-c", "cargo test -p tl3-controller --locked --test workers -- --maker-wait; a=$?; /home/rob/.local/bin/uv run --locked --project workers --python 3.14+gil python -m pytest workers/tests/test_pb_tests.py::test_maker_staged_submission_retains_checkout_parent -q; b=$?; exit $((a|b))"],
    }]
print(json.dumps(rows, indent=2))
