"""Seed one production mutant on a disposable BuildBuddy runner.

Python is verification glue: edit, make a scratch commit, and execute one proof.
The expected proof failure remains the run's nonzero verdict.
"""
import argparse
import pathlib
import subprocess

MUTANTS = {
    "marker": ("crates/github/src/marker_dedupe.rs", "delete_from: 1", "delete_from: 0", "//crates/github:verus_marker_dedupe"),
    "permanent": ("crates/controller/src/adapters/github/decide.rs", "PublicationRead::PrEvidence => Refusal::PermanentCompletion,", "PublicationRead::PrEvidence => Refusal::Pending,", "//crates/controller:verus_github_reconciliation"),
    "repost": ("crates/controller/src/adapters/github/decide.rs", "    !found\n", "    found\n", "//crates/controller:verus_github_reconciliation"),
    "hook": ("crates/controller/src/adapters/github/decide.rs", "    if outside_window {", "    if !outside_window {", "//crates/controller:verus_github_reconciliation"),
    "cursor": ("crates/controller/src/adapters/github/decide.rs", "    if candidate < after {", "    if candidate > after {", "//crates/controller:verus_github_reconciliation"),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("name", choices=MUTANTS)
    args = parser.parse_args()
    filename, before, after, target = MUTANTS[args.name]
    path = pathlib.Path(filename)
    source = path.read_text()
    # The production arm, after the contract. Never mutate the specification.
    if args.name == "permanent":
        start = source.index("pub fn publication_refusal(")
        prefix, body = source[:start], source[start:]
        assert body.count(before) == 1
        changed = prefix + body.replace(before, after)
    else:
        assert source.count(before) == 1
        changed = source.replace(before, after)
    path.write_text(changed)
    subprocess.run(["git", "add", filename], check=True)
    subprocess.run(["git", "-c", "user.name=CC-G mutant", "-c", "user.email=cc-g@example.invalid",
                    "-c", "commit.gpgsign=false", "commit", "-m", "Seed CC-G " + args.name + " mutant",
                    "-m", "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"], check=True)
    subprocess.run(["git", "rev-parse", "HEAD"], check=True)
    result = subprocess.run(["bazel", "test", "--config=remote", "--jobs=16",
                             "--noremote_accept_cached", target], check=False)
    raise SystemExit(result.returncode)


if __name__ == "__main__":
    main()
