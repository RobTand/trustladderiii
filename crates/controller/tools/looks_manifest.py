"""Print ticket 15's PrismaBuild verification campaign from a clean commit."""
import json
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parents[3]
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
proofs = json.loads(subprocess.check_output([
    sys.executable, str(root / "crates/kernel/run.py"), "manifest",
    "tests", "apply_upstream_stalled",
], text=True))
rows.extend(proofs)
print(json.dumps(rows, indent=2))
