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
PROOFS = ROOT / "crates/kernel/src/proofs.rs"
MUTANTS = {
    "apply_dependency_remove": (
        DEPENDENCIES,
        "d.node != node || d.target.kind != kind || d.target.number != number",
        "d.node != node",
    ),
    "progress_from_unplanned": (
        LIB, "if n.paused || !w.dependencies_delivered(NodeId(i as u64)) || !admits(w, n.repo)",
        "if true || n.paused || !w.dependencies_delivered(NodeId(i as u64)) || !admits(w, n.repo)",
    ),
    "progress_from_unplanned_delivered": (
        LIB, "if n.paused || !w.dependencies_delivered(NodeId(i as u64)) || !admits(w, n.repo)",
        "if true || n.paused || !w.dependencies_delivered(NodeId(i as u64)) || !admits(w, n.repo)",
    ),
    "progress_from_unplanned_waiting": (
        LIB, "!w.dependencies_delivered(NodeId(i as u64))", "false",
    ),
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
MUTANTS["progress_from_unplanned_delivered_one"] = MUTANTS["progress_from_unplanned_delivered"]
MUTANTS["progress_from_unplanned_waiting_one"] = MUTANTS["progress_from_unplanned_waiting"]


def main():
    harness = sys.argv[1]
    path, before, after = MUTANTS[harness]
    original = path.read_text()
    proof_source = PROOFS.read_text()
    if original.count(before) != 1:
        sys.exit("mutation no longer matches exactly once")
    print(f"RED {harness}: {before.strip()} -> {after.strip()}", flush=True)
    try:
        path.write_text(original.replace(before, after))
        if harness.startswith("progress_from_unplanned"):
            # A concrete consistent counterexample is enough for RED. Keep the
            # GREEN harnesses fully nondeterministic; only the mutant run uses
            # this witness, avoiding an expensive search for a known example.
            source = proof_source + """
fn unplanned_mutant_world(status: Option<DependencyStatus>, count: usize) -> World {
    let mut world = waiting_dependency_world();
    let mut other = Node::new(RepoId(0));
    other.phase = Phase::Closed(Outcome::Stopped);
    world.nodes.push(other);
    if let Some(status) = status {
        world.dependencies[0].status = status;
        if count == 2 {
            world.dependencies.push(Dependency {
                target: DependencyTarget { number: 2762, ..world.dependencies[0].target },
                status: DependencyStatus::Delivered,
                ..world.dependencies[0]
            });
        }
    } else {
        world.dependencies.clear();
        world.nodes[0].waiting_prerequisite = None;
        world.nodes[0].credit.withheld = false;
    }
    assert!(world.consistent());
    world
}
"""
            if harness == "progress_from_unplanned":
                source = source.replace("check_unplanned_progress(any_world(Some(Phase::Unplanned)))",
                                        "check_unplanned_progress(unplanned_mutant_world(None, 0))")
            elif harness.startswith("progress_from_unplanned_delivered"):
                for count in (1, 2):
                    source = source.replace(f"check_unplanned_progress(any_unplanned_dependencies::<{count}>(true))",
                                            f"check_unplanned_progress(unplanned_mutant_world(Some(DependencyStatus::Delivered), {count}))")
            else:
                source = source.replace("let world = any_unplanned_dependencies::<N>(false);",
                                        "let world = unplanned_mutant_world(Some(DependencyStatus::Pending), N);")
            PROOFS.write_text(source)
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
        PROOFS.write_text(proof_source)


if __name__ == "__main__":
    main()
