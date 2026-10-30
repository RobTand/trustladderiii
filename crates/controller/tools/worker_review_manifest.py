"""Print the worker verification campaign from a clean worktree.

Every row executes on PrismaBuild. Host tags cover worker, controller and store process tests.
"""
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("suite", choices=("workers", "planning", "maker", "workspace"))
parser.add_argument("--trial-image", help="immutable locally prepared harness image for an additional real maker trial")
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
commands = {
    "maker": ["cargo", "test", "-p", "tl3-controller", "--test", "workers", "--test", "github", "--test", "worktrees", "--locked"],
    "planning": ["bash", "crates/controller/tools/verify-planning.sh"],
    "workers": ["cargo", "test", "-p", "tl3-controller", "-p", "tl3-store", "--tests", "--locked"],
}
rows = [{
    "cwd": str(root), "argv": commands.get(args.suite, commands["workers"]),
    "cpus": 2, "demand": {"mem_gb": 6}, "tags": [host], "env": env, "priority": -10,
} for host in ("dl380g10", "sparky", "sparklina")]
workspace = {
    "cwd": str(root), "argv": ["bash", "crates/controller/tools/verify-workers.sh"],
    "cpus": 2, "demand": {"mem_gb": 6}, "anywhere": True, "env": env, "priority": -10,
}
if args.suite == "maker":
    rows.append({**workspace, "argv": ["bash", "crates/controller/tools/verify-maker.sh"]})
    if args.trial_image:
        rows.append({**workspace,
                     "argv": ["/home/rob/.local/bin/uv", "run", "--locked", "--project", "workers", "--python", "3.14+gil", "python", "crates/controller/tools/maker_trial.py", args.trial_image],
                     "env": {**env, "PYTHONPATH": "harness/src"},
                     "container_images": [args.trial_image], "progress_phases": ["tests=900"]})
print(json.dumps([workspace] if args.suite == "workspace" else rows, indent=2))
