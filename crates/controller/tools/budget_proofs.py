"""#780 standalone Kani campaign, with one rejecting mutant per property.

Only manifest construction runs locally. `remote HARNESS` runs a pinned Kani
bundle on a BuildBuddy cloud runner until #726 provides Bazel harness targets.
The prove/mutant entry points also support admitted PrismaBuild actions.
"""
import json
import hashlib
import os
from pathlib import Path
import re
import resource
import subprocess
import sys
import time
import tarfile
import tempfile
import urllib.request

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


def run(mode, name, verifier="kani"):
    source = SOURCE.with_name("cutoff.rs") if name == "cutoff_exact" else SOURCE
    original = source.read_text()
    if mode == "mutant":
        before, after = MUTANTS[name]
        if original.count(before) != 1:
            sys.exit("mutant does not match exactly once: " + name)
        source.write_text(original.replace(before, after))
    started = time.monotonic()
    try:
        proc = subprocess.run([verifier, "--output-format", "terse", "--harness", "proofs::" + name,
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


def remote(name):
    """Run one production harness and its mutant on a BuildBuddy cloud runner."""
    digest = "32e2b484d73ede0bbf64a2cf0879c4259422497d8aec4ee67d448de9ae7843d3"
    url = ("https://github.com/model-checking/kani/releases/download/kani-0.68.0/"
           "kani-0.68.0-x86_64-unknown-linux-gnu.tar.gz")
    os.environ.update(OMP_NUM_THREADS="1", RAYON_NUM_THREADS="1", CARGO_BUILD_JOBS="1")
    with tempfile.TemporaryDirectory(prefix="tl3-budget-kani-") as directory:
        rustc_digest = "e531815fa551362b5a485e0d286b6c96555312d49359c0b8fe9e8f34c45594b4"
        rustc_url = ("https://static.rust-lang.org/dist/2026-08-21/"
                     "rustc-nightly-x86_64-unknown-linux-gnu.tar.xz")
        for filename, source_url, expected in [("kani.tar.gz", url, digest),
                                               ("rustc.tar.xz", rustc_url, rustc_digest)]:
            archive = Path(directory) / filename
            with urllib.request.urlopen(source_url, timeout=120) as response, archive.open("wb") as stream:
                while chunk := response.read(1024 * 1024):
                    stream.write(chunk)
            with archive.open("rb") as stream:
                if hashlib.file_digest(stream, "sha256").hexdigest() != expected:
                    raise RuntimeError("proof toolchain checksum mismatch")
            with tarfile.open(archive) as bundle:
                bundle.extractall(directory, filter="data")
        kani = Path(directory) / "kani-0.68.0"
        if (kani / "rust-toolchain-version").read_text().strip() != "nightly-2026-08-21-x86_64-unknown-linux-gnu":
            raise RuntimeError("Kani compiler toolchain mismatch")
        toolchain = Path(directory) / "rustc-nightly-x86_64-unknown-linux-gnu/rustc"
        (kani / "toolchain").symlink_to(toolchain, target_is_directory=True)
        os.environ["LD_LIBRARY_PATH"] = str(toolchain / "lib")
        os.environ["PATH"] = str(kani / "bin") + os.pathsep + os.environ.get("PATH", os.defpath)
        verifier = str(kani / "bin/kani-driver")
        print(json.dumps({"kani": "0.68.0", "archive_sha256": digest,
                          "rustc_archive_sha256": rustc_digest}), flush=True)
        if run("prove", name, verifier):
            return 1
        return run("mutant", name, verifier)


if __name__ == "__main__":
    if sys.argv[1:] == ["manifest"]:
        manifest()
    elif len(sys.argv) == 3 and sys.argv[1] == "remote" and sys.argv[2] in MUTANTS:
        sys.exit(remote(sys.argv[2]))
    elif len(sys.argv) == 3 and sys.argv[1] in {"prove", "mutant"} and sys.argv[2] in MUTANTS:
        sys.exit(run(*sys.argv[1:]))
    else:
        sys.exit(__doc__)
