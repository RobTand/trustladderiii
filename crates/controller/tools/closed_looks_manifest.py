"""Print ticket 229's PrismaBuild actions from a clean detached checkout."""
import argparse
import json
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("selection", choices=["red", "hosts", "workspace"])
args = parser.parse_args()
root = Path(__file__).resolve().parents[3]
if subprocess.check_output(["git", "status", "--porcelain"], cwd=root, text=True):
    raise SystemExit("Commit changes before submitting.")
if subprocess.run(["git", "symbolic-ref", "-q", "HEAD"], cwd=root,
                  stdout=subprocess.DEVNULL).returncode == 0:
    raise SystemExit("Use git worktree add --detach at the commit to verify.")
env = {
    "PATH": "/home/rob/.cargo/bin:/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin",
    "RUSTUP_HOME": "/home/rob/.rustup", "CARGO_HOME": "/home/rob/.cargo",
    "KANI_HOME": "/home/rob/.kani", "CARGO_BUILD_JOBS": "2", "RUST_TEST_THREADS": "2",
    "OMP_NUM_THREADS": "1", "MKL_NUM_THREADS": "1", "OPENBLAS_NUM_THREADS": "1",
    "RAYON_NUM_THREADS": "1", "TL3_TEST_UV": "/home/rob/.local/bin/uv",
    "UV_CONCURRENT_DOWNLOADS": "2", "UV_CONCURRENT_BUILDS": "2",
}
base = {"cwd": str(root), "cpus": 2, "demand": {"mem_gb": 6},
        "priority": -10, "timeout_s": 3600, "env": env}
if args.selection == "red":
    rows = [dict(base, anywhere=True, argv=["cargo", "test", "-p", "tl3-controller",
            "--locked", "--test", "looks", "--", "--closed-looks"])]
elif args.selection == "hosts":
    rows = [dict(base, tags=[host], argv=["cargo", "test", "-p", "tl3-controller",
            "--locked", "--all-targets"])
            for host in ["dl380g10", "sparky", "sparklina"]]
else:
    rows = [dict(base, anywhere=True, argv=["bash", "crates/controller/tools/verify.sh"])]
print(json.dumps(rows, indent=2))
