"""Describe catch-up recovery checks for PrismaBuild without running them."""

import json
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parents[3]
if subprocess.check_output(["git", "status", "--porcelain"], cwd=root, text=True):
    raise SystemExit("Commit changes before submitting.")
mode = sys.argv[1] if len(sys.argv) > 1 else "hosts"
env = {
    "PATH": "/home/rob/.cargo/bin:/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin",
    "RUSTUP_HOME": "/home/rob/.rustup",
    "CARGO_HOME": "/home/rob/.cargo",
    "KANI_HOME": "/home/rob/.kani",
    "CARGO_BUILD_JOBS": "2",
    "RUST_TEST_THREADS": "2",
    "OMP_NUM_THREADS": "1",
    "MKL_NUM_THREADS": "1",
    "OPENBLAS_NUM_THREADS": "1",
    "TMPDIR": "/home/rob/tmp",
    "TL3_TEST_UV": "/home/rob/.local/bin/uv",
}
base = {
    "cwd": str(root),
    "cpus": 2,
    "demand": {"mem_gb": 6},
    "priority": -10,
    "timeout_s": 3600,
    "env": env,
}
if mode == "red":
    rows = [dict(base, anywhere=True, argv=["bash", "-c", '''status=0
cargo test -p tl3-controller --locked --test github -- --catchup-heads || status=1
cargo test -p tl3-controller --locked --test mergify -- --failed-catchup-wake || status=1
exit "$status"
'''])]
elif mode == "hosts":
    rows = [dict(base, tags=[host], argv=[
        "cargo", "test", "-p", "tl3-controller", "--locked", "--lib",
        "--test", "github", "--test", "mergify", "--test", "publishing",
        "--test", "simulation",
        "--test", "crash_safety", "--test", "ingress_loom",
    ]) for host in ("dl380g10", "sparky", "sparklina")]
elif mode == "workspace":
    rows = [dict(base, anywhere=True, argv=["bash", "crates/controller/tools/verify.sh"])]
else:
    raise SystemExit("usage: catchup_heads_manifest.py [red|hosts|workspace]")
print(json.dumps(rows, indent=2))
