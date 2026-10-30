"""Print the bounded loom, three-host crash, and workspace verification actions.

Run from a clean final commit. Submit this output with the published pbcampaign.
"""
import json
from pathlib import Path

root = Path(__file__).resolve().parents[3]
env = {
    "PATH": "/home/rob/.cargo/bin:/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin",
    "RUSTUP_HOME": "/home/rob/.rustup",
    "CARGO_HOME": "/home/rob/.cargo",
    "KANI_HOME": "/home/rob/.kani",
    "CARGO_BUILD_JOBS": "2",
    "RUST_TEST_THREADS": "1",
    "OMP_NUM_THREADS": "1",
    "MKL_NUM_THREADS": "1",
    "OPENBLAS_NUM_THREADS": "1",
    "RAYON_NUM_THREADS": "1",
    "TL3_SIM_SEED": "1450022",
}
rows = []
for host in ["dl380g10", "sparky", "sparklina"]:
    command = "bash crates/controller/tools/verify-loom.sh && cargo test -p tl3-controller --test crash_safety --locked"
    if host == "dl380g10":
        command += " && for iteration in {1..50}; do TL3_LOOM_BOUND=2 cargo test -p tl3-controller --features loom --lib --locked loom_bell_ -- --test-threads=1 || exit; done"
    rows.append({
        "cwd": str(root), "tags": [host], "cpus": 2,
        "demand": {"mem_gb": 6}, "env": env,
        "priority": -10, "timeout_s": 3600,
        "argv": ["bash", "-c", command],
    })
rows.append({
    "cwd": str(root), "anywhere": True, "cpus": 2,
    "demand": {"mem_gb": 6}, "env": env,
    "priority": -10, "timeout_s": 3600,
    "argv": ["bash", "crates/controller/tools/verify.sh"],
})
rows.append({
    "cwd": str(root), "anywhere": True, "cpus": 2,
    "demand": {"mem_gb": 6}, "env": env,
    "priority": -10, "timeout_s": 3600,
    "argv": ["bash", "-c", "for mutant in ingress wake-before-durable consume-before-apply no-owner-drain gap stop-dropped stop-after-wake stop-relaxed progress persistence registration awaited reset-outage; do python3 crates/controller/tools/loom_mutants.py \"$mutant\" || exit; done"],
})
print(json.dumps(rows, indent=2))
