"""Run one ordering mutant inside a PrismaBuild snapshot, or print its diff.

The caller supplies a clean scratch checkout. This command refuses dirty trees
and restores the original source even after a failed test.
"""
import argparse
import difflib
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[3]
MUTANTS = {
    "persistence": (
        "crates/controller/src/adapters/workers/source.rs",
        '                    store.record_worker_progress(\n                        &delivery.key,\n                        *number,\n                        run.offset\n                            .try_into()\n                            .map_err(|_| Error::Invalid("worker output offset overflow"))?,\n                        value["unit"].as_str().unwrap(),\n                        wait.as_ref(),\n                    )?;',
        "                    // Mutant: publish without recording durable progress.",
        "loom_worker_", "progress published before persistence",
    ),
    "ingress": (
        "crates/controller/src/ingress.rs",
        "            drain(controller)?;\n            apply(controller, input)?;\n            controller.look_due()?;",
        "            drain(controller)?;\n            let _ = input;\n            controller.look_due()?;",
        "loom_sources_", "deadlock",
    ),
    "gap": (
        "crates/controller/src/ingress.rs",
        "        let _ = self.0.sender.send(WebhookMessage::Gap);",
        "        // Mutant: lose the tunnel gap notification.",
        "loom_webhook_", "gap lost or reordered",
    ),
    "progress": (
        "crates/controller/src/adapters/workers/source.rs",
        "                    && run.offset > recorded",
        "                    && run.offset > 0",
        "loom_worker_", "restart doubled progress",
    ),
    "registration": (
        "crates/controller/src/adapters/prismabuild/source.rs",
        "                    self.waits.insert(wait.delivery.key.clone(), wait);",
        "                    let _ = wait;",
        "loom_bell_", "declared wait registration lost",
    ),
    "awaited": (
        "crates/controller/src/adapters/prismabuild/source.rs",
        '.parse::<ActionKey>()\n                    .expect("validated wait key")',
        '.replace(\'a\', "b").parse::<ActionKey>()\n                    .expect("validated wait key")',
        "loom_bell_", "reset/outage lost ending",
    ),
    "reset-outage": (
        "crates/prismabuild/src/bell.rs",
        ".reconcile(if reset { awaited } else { &hinted })",
        ".reconcile(&hinted)",
        "loom_bell_", "reset/outage lost ending",
    ),
}


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("mutant", choices=MUTANTS)
    parser.add_argument("--diff", action="store_true")
    args = parser.parse_args()
    name, old, new, test, expected = MUTANTS[args.mutant]
    path = ROOT / name
    source = path.read_text()
    if source.count(old) != 1:
        raise SystemExit(f"{name}: expected exactly one mutation site")
    mutated = source.replace(old, new)
    print("".join(difflib.unified_diff(source.splitlines(True), mutated.splitlines(True), fromfile=f"a/{name}", tofile=f"b/{name}")), flush=True)
    if args.diff:
        return
    if subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=no"], cwd=ROOT).strip():
        raise SystemExit("mutants require a clean scratch checkout")
    path.write_text(mutated)
    try:
        result = subprocess.run(
            ["cargo", "test", "-p", "tl3-controller", "--lib", "--locked", test, "--", "--test-threads=1", "--nocapture"],
            cwd=ROOT, env={**os.environ, "TL3_LOOM": "1"}, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        )
        print(result.stdout, flush=True)
        assert result.returncode != 0 and expected in result.stdout, f"mutant {args.mutant} did not fail at {expected!r}, exit {result.returncode}"
        print(f"RED VERIFIED mutant={args.mutant} test={test} exit={result.returncode} assertion={expected}", flush=True)
    finally:
        path.write_text(source)


if __name__ == "__main__":
    main()
