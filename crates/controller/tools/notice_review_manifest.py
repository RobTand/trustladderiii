"""Print ticket 87's PrismaBuild actions from a clean commit."""
import argparse
import json
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("suite", choices=("red", "hosts", "workspace", "all"))
args = parser.parse_args()
root = Path(__file__).resolve().parents[3]
if subprocess.check_output(["git", "status", "--porcelain"], cwd=root, text=True):
    raise SystemExit("Commit changes before submitting.")
if subprocess.run(["git", "symbolic-ref", "-q", "HEAD"], cwd=root,
                  stdout=subprocess.DEVNULL).returncode == 0:
    raise SystemExit("Use git worktree add --detach at the commit to verify.")
common = {
    "cwd": str(root), "cpus": 2, "demand": {"mem_gb": 8},
    "env": {
        "PATH": "/home/rob/.cargo/bin:/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin",
        "CARGO_HOME": "/home/rob/.cargo", "RUSTUP_HOME": "/home/rob/.rustup",
        "KANI_HOME": "/home/rob/.kani", "CARGO_BUILD_JOBS": "2",
        "RUST_TEST_THREADS": "2", "OMP_NUM_THREADS": "1",
        "RAYON_NUM_THREADS": "1", "OPENBLAS_NUM_THREADS": "1", "MKL_NUM_THREADS": "1",
        "TL3_TEST_UV": "/home/rob/.local/bin/uv",
        "UV_CONCURRENT_DOWNLOADS": "2", "UV_CONCURRENT_BUILDS": "2",
    },
    "priority": -10,
}
rows = []
if args.suite in ("hosts", "all"):
    command = ["cargo", "test", "-p", "tl3-controller", "--locked"]
    for target in ("github", "looks"):
        command.extend(["--test", target])
    rows = [{**common, "tags": [host], "argv": command}
            for host in ("dl380g10", "sparky", "sparklina")]
if args.suite in ("red", "workspace", "all"):
    command = (["bash", "crates/controller/tools/verify.sh"] if args.suite in ("workspace", "all")
               else ["cargo", "test", "-p", "tl3-controller", "--test", "github", "--locked",
                     "--", "--notice-incidents"])
    rows.append({**common, "anywhere": True, "argv": command})
print(json.dumps(rows, indent=2))
