"""Run the Mergify queue decisions' Kani proofs, or their seeded mutants (#735).

The remaining harnesses live in tl3-mergify (`src/decide/enqueue_proofs.rs`).

    kani.py manifest [NAME...]   print a PrismaBuild campaign manifest
    kani.py prove [NAME...]      prove the named harnesses, or all of them
    kani.py verify [NAME...]     reject mutants, then prove the unmodified source
    kani.py mutants [NAME...]    for each harness, seed each of its mutants and require
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

ROOT = pathlib.Path(__file__).resolve().parents[3]
DECIDE = "crates/mergify/src/decide/"
STATUS = "crates/github/src/status.rs"
# Each proof file, its package and the module path of its harnesses.
PROOF_FILES = [
    (DECIDE + "enqueue_proofs.rs", "tl3-mergify", "decide::enqueue_proofs"),
]
# Seeded mutants: (harness, file, exact text, replacement). Each breaks the
# property its harness states, so the harness must fail. Every harness has at
# least one.
MUTANTS = [
    ("enqueue_ack_sound", DECIDE + "enqueue.rs", "let Some(recovery) = issue.requeue_recovery else", "let Some(recovery) = issue.recovery else"),
    ("enqueue_ack_sound", DECIDE + "enqueue.rs", "before.requeue.is_none() && read.check_changed", "read.check_changed"),
    ("enqueue_stale_head", DECIDE + "enqueue.rs", "read.head != Head::Current\n                || !read.open", "!read.open"),
    ("enqueue_refused_sticky", DECIDE + "enqueue.rs", "if refused_intent {", "if false && refused_intent {"),
    ("enqueue_write_ahead", DECIDE + "enqueue.rs", "out.push(Effect::SaveIntent);\n                out.push(if intent.requeue.is_some()", "out.push(if intent.requeue.is_some()"),
    ("enqueue_write_ahead", DECIDE + "enqueue.rs", "out.push(Effect::SaveIntent);\n                out.push(if intent.requeue.is_some() {\n                    Effect::Read(Need::Command)\n                } else {\n                    Effect::Read(Need::LabelPull)\n                });", "out.push(if intent.requeue.is_some() {\n                    Effect::Read(Need::Command)\n                } else {\n                    Effect::Read(Need::LabelPull)\n                });\n                out.push(Effect::SaveIntent);"),
    ("enqueue_stale_head", DECIDE + "enqueue.rs", "} else if head != Head::Current {\n                Effect::Complete(Complete::Retryable)", "} else if false && head != Head::Current {\n                Effect::Complete(Complete::Retryable)"),
    ("enqueue_recovery_grace", DECIDE + "enqueue.rs", "                    now_ms.saturating_sub(since) >= GRACE_MS", "                    now_ms.saturating_sub(since) >= GRACE_MS / 2"),
    ("enqueue_recovery_grace", DECIDE + "enqueue.rs", "                || read.paused\n", "                || read.paused\n                || read.waiting && !recovery.eligible_since_ms.is_some_and(|since| read.now_ms.saturating_sub(since) >= GRACE_MS)\n"),
    ("enqueue_dequeue_fresh", DECIDE + "enqueue.rs", "event.id > queued", "event.id >= queued"),
    ("enqueue_required_check", DECIDE + "enqueue.rs", "check.app == App::Actions", "check.app != App::Missing"),
    ("enqueue_ack_sound", DECIDE + "enqueue.rs", "state.confirmed = true;", "// mutant: omit acknowledgement state"),
]


def harnesses():
    """Each harness name, with its package and full harness path."""
    found = {}
    for path, package, module in PROOF_FILES:
        source = (ROOT / path).read_text()
        names = re.findall(r"^proof!\(\s*(\w+),", source, re.M)
        names += re.findall(r"#\[kani::proof\]\s*#\[kani::unwind\(\d+\)\]\s*fn (\w+)\(", source)
        for name in names:
            if name in found:
                sys.exit(f"duplicate harness: {name}")
            found[name] = (package, f"{module}::{name}")
    return found


def kani(package, paths):
    """Run Kani on harnesses of one package; return (exit code, output)."""
    argv = ["cargo", "kani", "-p", package, "--output-format", os.environ.get("KANI_OUTPUT", "terse"),
            "-j", os.environ.get("KANI_JOBS", "1")]
    for path in paths:
        argv += ["--harness", path, "--exact"]
    print(f"== kani: {' '.join(argv)}", flush=True)
    start = time.monotonic()
    proc = subprocess.Popen(argv, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    lines = []
    for line in proc.stdout:
        print(line, end="", flush=True)
        lines.append(line)
    code = proc.wait()
    print(f"== kani: exit {code} in {time.monotonic() - start:.1f}s", flush=True)
    return code, "".join(lines)


def prove(names, known):
    failed = 0
    for package in sorted({known[name][0] for name in names}):
        paths = [known[name][1] for name in names if known[name][0] == package]
        code, output = kani(package, paths)
        if code:
            failed = code
            continue
        # A proof whose cover checks aren't all reachable proved nothing about
        # those cases, so it fails too.
        covers = re.findall(r"\*\* (\d+) of (\d+) cover properties satisfied", output)
        if len(covers) != len(paths):
            print(f"== kani: {len(covers)} cover reports for {len(paths)} harnesses", flush=True)
            failed = 1
        elif any(int(hit) < int(total) for hit, total in covers):
            print("== kani: a cover check was not satisfied, so the proof may be vacuous", flush=True)
            failed = 1
    return failed


def mutants(names, known):
    failed = []
    for name, file, before, after in MUTANTS:
        if name not in names:
            continue
        path = ROOT / file
        original = path.read_text()
        if original.count(before) != 1:
            print(f"MUTANT {name}: the seeded text no longer matches exactly once", flush=True)
            failed.append(name)
            continue
        print(f"MUTANT {name}: {file}: {before.strip()!r} -> {after.strip()!r}", flush=True)
        try:
            path.write_text(original.replace(before, after))
            package, harness = known[name]
            code, output = kani(package, [harness])
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
    rows = [{"cwd": str(ROOT), "argv": ["/usr/bin/python3", "crates/mergify/tools/kani.py", "prove", name],
             "cpus": 1, "demand": {"mem_gb": 2}, "anywhere": True, "env": env,
             "priority": -10, "timeout_s": 1800} for name in names]
    print(json.dumps(rows, indent=2))
    return 0


def main():
    mode, names = (sys.argv[1], sys.argv[2:]) if len(sys.argv) > 1 else ("", [])
    known = harnesses()
    if set(names) - set(known):
        sys.exit(f"unknown harness: {sorted(set(names) - set(known))}")
    if set(known) != {mutant[0] for mutant in MUTANTS}:
        sys.exit("every harness needs a seeded mutant, and every mutant a harness")
    names = names or sorted(known)
    if mode == "manifest":
        return manifest(names)
    if mode == "prove":
        return prove(names, known)
    if mode == "mutants":
        return mutants(names, known)
    if mode == "verify":
        return mutants(names, known) or prove(names, known)
    sys.exit(__doc__)


if __name__ == "__main__":
    sys.exit(main())
