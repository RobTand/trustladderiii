"""Print ticket #16's required five-placement PrismaBuild campaign.

Run from a clean commit and keep the output outside the checkout.
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
}
rows = []
for host, seed, mode in [
    ("dl380g10", 1450022, "smoke"),
    ("sparky", 1450022, "smoke"),
    ("sparklina", 1450022, "smoke"),
    (None, 1450022, "full"),
    (None, 1450023, "verify"),
]:
    row = {
        "cwd": str(root),
        "argv": ["bash", "crates/controller/tools/verify.sh"] if mode == "verify" else [
            "cargo", "test", "-p", "tl3-controller",
            *(["--test", "crash_safety"] if mode == "full" else ["--all-targets"]),
            "--locked",
        ],
        "cpus": 2,
        "demand": {"mem_gb": 6},
        "env": {
            **env,
            "TL3_SIM_SEED": str(seed),
            "TL3_SIM_FULL": "1" if mode == "full" else "0",
        },
        "priority": -10,
        "timeout_s": 3600,
    }
    if host:
        row["tags"] = [host]
    else:
        row["anywhere"] = True
    rows.append(row)
print(json.dumps(rows, indent=2))
