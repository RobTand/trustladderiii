"""Generate Q45's final PrismaBuild campaign from a clean commit.

The host rows exercise process and socket behavior once on each CI machine.
The kernel's run.py manifest supplies every proof as a separate portable action.
"""
import argparse
import json
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("suite", choices=("hosts", "python", "workspace", "kernel", "all"))
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
common = {"cwd": str(root), "cpus": 2, "demand": {"mem_gb": 8}, "env": env}
rows = []
if args.suite in {"hosts", "all"}:
    # Include every process/socket/crash fixture, alongside the required scenario families.
    rows.extend({**common, "tags": [host], "argv": [
        "cargo", "test", "--workspace", "--all-targets", "--locked",
    ]} for host in ("dl380g10", "sparky", "sparklina"))
if args.suite in {"python", "all"}:
    rows.append({**common, "cpus": 4, "anywhere": True, "argv": [
        "/usr/bin/python3.14", "-c", """
import os
from pathlib import Path
import subprocess
subprocess.run(["cargo", "build", "--locked", "-p", "tl3-controller", "--example", "tl3-deploy"], check=True)
os.environ["TL3_DEPLOY_HELPER"] = str(Path.cwd() / "target/debug/examples/tl3-deploy")
subprocess.run(["/usr/bin/python3.14", "tools/ci/check.py", "python"], check=True)
"""]})
if args.suite in {"workspace", "all"}:
    rows.append({**common, "anywhere": True,
                 "argv": ["bash", "crates/controller/tools/verify.sh"]})
if args.suite in {"kernel", "all"}:
    kernel = json.loads(subprocess.check_output([
        "/usr/bin/python3", str(root / "crates/kernel/run.py"), "manifest",
    ], text=True))
    for row in kernel:
        row["env"].update({"TMPDIR": "/home/rob/tmp"})
    rows.extend(kernel)
print(json.dumps(rows, indent=2))
