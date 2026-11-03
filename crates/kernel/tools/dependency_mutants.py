"""Demonstrate that each #693 proof detects removal of its dependency rule.

Run only on admitted compute or BuildBuddy step mode, in a disposable checkout:
    python3 crates/kernel/tools/dependency_mutants.py HARNESS
The original source is restored even when Kani fails.
"""
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
DEPENDENCIES = ROOT / "crates/kernel/src/dependencies.rs"
LIB = ROOT / "crates/kernel/src/lib.rs"
MUTANTS = {
    "dependency_no_cycle": (
        DEPENDENCIES,
        """        let rejected = status == DependencyStatus::Pending
            && target
                .request
                .is_some_and(|target| self.dependency_path(target, node));""",
        "        let rejected = false;",
    ),
    "dependency_waiting_holds_nothing": (
        LIB, "!w.dependencies_delivered(NodeId(i as u64))", "false",
    ),
    "dependency_delivered_releases": (
        DEPENDENCIES, """        if target.number == 0""",
        """        if status == DependencyStatus::Delivered { return; }
        if target.number == 0""",
    ),
    "dependency_abandoned_never_releases": (
        DEPENDENCIES,
        "previous.rejected || previous.status != DependencyStatus::Pending",
        "previous.rejected || previous.status == DependencyStatus::Delivered",
    ),
    "apply_dependency": (
        DEPENDENCIES, """        if target.number == 0""",
        """        if self.nodes[i].phase.is_closed() { return; }
        if target.number == 0""",
    ),
}


def main():
    harness = sys.argv[1]
    path, before, after = MUTANTS[harness]
    original = path.read_text()
    if original.count(before) != 1:
        sys.exit("mutation no longer matches exactly once")
    print(f"RED {harness}: {before.strip()} -> {after.strip()}", flush=True)
    try:
        path.write_text(original.replace(before, after))
        result = subprocess.run(
            ["cargo", "kani", "-p", "tl3-kernel", "--output-format", "terse",
             "--harness", f"proofs::{harness}", "--exact"],
            cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        )
        print(result.stdout, flush=True)
        if (result.returncode == 0 or "VERIFICATION:- FAILED" not in result.stdout
                or "out of memory" in result.stdout.lower()
                or "unwinding assertion" in result.stdout.lower()):
            sys.exit("mutation did not produce a valid failing proof")
        print(f"RED witnessed: proofs::{harness}: VERIFICATION:- FAILED", flush=True)
    finally:
        path.write_text(original)


if __name__ == "__main__":
    main()
