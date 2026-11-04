"""Run the Mergify core's Kani proofs, or their seeded mutants (#735).

    kani.py manifest [NAME...]   print a PrismaBuild campaign manifest
    kani.py prove [NAME...]      prove the named harnesses, or all of them
    kani.py mutants [NAME...]    for each harness, seed its mutant and require
                                 that the harness FAILS; the source is restored

Run `prove` and `mutants` only inside an admitted action, from a disposable
checkout. Until #726 gives each harness a Bazel target, proofs run on
PrismaBuild as crates/kernel/run.py does. A proof passes only when Kani
verifies it and every cover check is satisfied, so no proof passes vacuously.
"""
import json
import os
import pathlib
import re
import subprocess
import sys
import time

CRATE = pathlib.Path(__file__).resolve().parents[1]
SRC = CRATE / "src"
# One mutation per harness: (file, exact text, replacement). Each breaks the
# property its harness states, so the harness must fail.
MUTANTS = {
    "observe_select": ("observe.rs", "check.id >= checks[best].id", "check.id > checks[best].id"),
    "observe_merged": (
        "observe.rs",
        "if reported == QueueState::Merged {\n        QueueState::Unknown",
        "if reported == QueueState::Waiting {\n        QueueState::Unknown",
    ),
    "observe_conflict": (
        "observe.rs",
        "(pull.mergeable == Some(false) || pull.mergeable_state == Mergeable::Dirty)",
        "pull.mergeable == Some(false)",
    ),
    "observe_rule_concluded": ("observe.rs", "finished >= dequeued", "finished > dequeued"),
    "observe_check_state": (
        "observe.rs",
        "    ) {\n        QueueState::FailedCi",
        "    ) && conclusion.is_none() {\n        QueueState::FailedCi",
    ),
    "checks_passed_total": (
        "facts.rs",
        "|a, b| a.app_key == b.app_key && a.name_key == b.name_key",
        "|a, b| a.name_key == b.name_key",
    ),
    "queue_blocked_total": ("facts.rs", "            && !check.required\n", "\n"),
    "q52": (
        "translate.rs",
        "Kind::CheckMissing if node.enqueuing() && approved_head",
        "Kind::CheckMissing if approved_head",
    ),
    "translate_closed": (
        "translate.rs",
        "(!matches!(kind, Kind::Merged { .. }) || node.merged)",
        "!matches!(kind, Kind::Merged { .. })",
    ),
    "translate_head_ci": (
        "translate.rs",
        "return node.phase == Phase::Queued || node.enqueuing();",
        "return node.phase != Phase::Closed;",
    ),
    "translate_route": (
        "translate.rs",
        "if kind.names_attempt_head() && !head_current {",
        "if kind.names_attempt_head() && !head_current && approved_head {",
    ),
    "ci_classify": ("translate.rs", "outside && !repeated", "outside"),
    "person_facts_total": (
        "facts.rs",
        "if event.actor != Actor::Person {",
        "if event.actor == Actor::Missing {",
    ),
    "http_status_class_total": (
        "http.rs",
        "status == 429 || status == 403 && rate_signal",
        "status == 429 || status == 403",
    ),
    "facts_outcome": ("facts.rs", "head_infra.unwrap_or(true)", "head_infra.unwrap_or(false)"),
}


def harnesses():
    return re.findall(r"^proof!\(\s*(\w+),", (SRC / "proofs.rs").read_text(), re.M)


def kani(names):
    """Run Kani on the named harnesses; return (exit code, output)."""
    argv = ["cargo", "kani", "-p", "tl3-mergify-core", "--output-format", os.environ.get("KANI_OUTPUT", "terse"),
            "-j", os.environ.get("KANI_JOBS", "1")]
    for name in names:
        argv += ["--harness", f"proofs::{name}", "--exact"]
    print(f"== kani: {' '.join(argv)}", flush=True)
    start = time.monotonic()
    proc = subprocess.Popen(argv, cwd=CRATE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    lines = []
    for line in proc.stdout:
        print(line, end="", flush=True)
        lines.append(line)
    code = proc.wait()
    print(f"== kani: exit {code} in {time.monotonic() - start:.1f}s", flush=True)
    return code, "".join(lines)


def prove(names):
    code, output = kani(names)
    if code:
        return code
    # A proof whose cover checks aren't all reachable proved nothing about
    # those cases, so it fails too.
    covers = re.findall(r"\*\* (\d+) of (\d+) cover properties satisfied", output)
    if len(covers) != len(names):
        print(f"== kani: {len(covers)} cover reports for {len(names)} harnesses", flush=True)
        return 1
    if any(int(hit) < int(total) for hit, total in covers):
        print("== kani: a cover check was not satisfied, so the proof may be vacuous", flush=True)
        return 1
    return 0


def mutants(names):
    failed = []
    for name in names:
        file, before, after = MUTANTS[name]
        path = SRC / file
        original = path.read_text()
        if original.count(before) != 1:
            print(f"MUTANT {name}: the seeded text no longer matches exactly once", flush=True)
            failed.append(name)
            continue
        print(f"MUTANT {name}: {file}: {before.strip()!r} -> {after.strip()!r}", flush=True)
        try:
            path.write_text(original.replace(before, after))
            code, output = kani([name])
        finally:
            path.write_text(original)
        rejected = code != 0 and "VERIFICATION:- FAILED" in output
        print(f"MUTANT {name}: {'REJECTED' if rejected else 'SURVIVED'}", flush=True)
        if not rejected:
            failed.append(name)
    if failed:
        print(f"== mutants that survived or no longer apply: {failed}", flush=True)
    return 1 if failed else 0


def manifest(names):
    env = {
        "PATH": "/home/rob/.cargo/bin:/usr/local/bin:/usr/bin:/bin",
        "RUSTUP_HOME": "/home/rob/.rustup", "CARGO_HOME": "/home/rob/.cargo",
        "KANI_HOME": "/home/rob/.kani", "CARGO_BUILD_JOBS": "1",
        "KANI_JOBS": "1", "OMP_NUM_THREADS": "1", "RAYON_NUM_THREADS": "1",
    }
    rows = [{"cwd": str(CRATE), "argv": ["/usr/bin/python3", "tools/kani.py", "prove", name],
             "cpus": 1, "demand": {"mem_gb": 4}, "anywhere": True, "env": env,
             "priority": -10, "timeout_s": 1800} for name in names]
    print(json.dumps(rows, indent=2))
    return 0


def main():
    mode, names = (sys.argv[1], sys.argv[2:]) if len(sys.argv) > 1 else ("", [])
    known = harnesses()
    if set(names) - set(known):
        sys.exit(f"unknown harness: {sorted(set(names) - set(known))}")
    if set(known) != set(MUTANTS):
        sys.exit("every harness needs exactly one seeded mutant")
    names = names or known
    if mode == "manifest":
        return manifest(names)
    if mode == "prove":
        return prove(names)
    if mode == "mutants":
        return mutants(names)
    sys.exit(__doc__)


if __name__ == "__main__":
    sys.exit(main())
