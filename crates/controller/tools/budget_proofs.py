"""#780 standalone Kani campaign, with one rejecting mutant per property.

Only manifest construction runs locally. Run prove/mutant inside an admitted
PrismaBuild action until #726's per-harness BuildBuddy mechanism is available.
"""
import json
from pathlib import Path
import re
import resource
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[3]
SOURCE = ROOT / "crates/controller/src/budget/decision.rs"
MUTANTS = {
    "fit_monotone": ("size < limit", "size != limit"),
    "budget_bounds": ("allowed.saturating_sub(used)", "allowed.wrapping_sub(used)"),
    "cutoff_exact": ("* 100 >=", "* 100 >"),
    "compaction_reduces": ("after < before", "after <= before"),
    "reserve_preserved": ("maker\n        &&", "maker\n        ||"),
    "exhaustion_exact": ("if turns >= allowed", "if turns > allowed"),
}


def manifest():
    env = {"PATH": "/home/rob/.cargo/bin:/usr/local/bin:/usr/bin:/bin",
           "RUSTUP_HOME": "/home/rob/.rustup", "CARGO_HOME": "/home/rob/.cargo",
           "KANI_HOME": "/home/rob/.kani", "CARGO_BUILD_JOBS": "1",
           "OMP_NUM_THREADS": "1", "RAYON_NUM_THREADS": "1"}
    rows = [{"cwd": str(ROOT), "argv": ["/usr/bin/python3", "crates/controller/tools/budget_proofs.py", mode, name],
             "cpus": 1, "demand": {"mem_gb": 2}, "anywhere": True,
             "priority": -10, "timeout_s": 600, "env": env}
            for name in MUTANTS for mode in ("prove", "mutant")]
    print(json.dumps(rows, indent=2))


def run(mode, name):
    source = SOURCE.with_name("cutoff.rs") if name == "cutoff_exact" else SOURCE
    original = source.read_text()
    if mode == "mutant":
        before, after = MUTANTS[name]
        if original.count(before) != 1:
            sys.exit("mutant does not match exactly once: " + name)
        source.write_text(original.replace(before, after))
    started = time.monotonic()
    try:
        proc = subprocess.run(["kani", "--output-format", "terse", "--harness", "proofs::" + name,
                               "--exact", "crates/controller/src/budget/kani.rs"],
                              cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    finally:
        source.write_text(original)
    print(proc.stdout)
    covers = re.findall(r"\*\* (\d+) of (\d+) cover properties satisfied", proc.stdout)
    passed = (proc.returncode == 0 and len(covers) == 1 and
              all(int(hit) == int(total) and int(total) > 0 for hit, total in covers))
    rejected = proc.returncode != 0 and "VERIFICATION:- FAILED" in proc.stdout
    success = passed if mode == "prove" else rejected
    print(json.dumps({"harness": name, "mode": mode, "passed": success, "exit": proc.returncode,
                      "covers": covers, "seconds": time.monotonic() - started,
                      "max_rss_kib": resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss}))
    return 0 if success else 1


if __name__ == "__main__":
    if sys.argv[1:] == ["manifest"]:
        manifest()
    elif len(sys.argv) == 3 and sys.argv[1] in {"prove", "mutant"} and sys.argv[2] in MUTANTS:
        sys.exit(run(*sys.argv[1:]))
    else:
        sys.exit(__doc__)
