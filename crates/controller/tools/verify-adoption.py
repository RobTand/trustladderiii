"""Rerun Q45's focused behavior checks inside one admitted PrismaBuild action."""
import argparse
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--proofs", action="store_true")
args = parser.parse_args()
commands = [
    ["cargo", "test", "--locked", "-p", "tl3-kernel", "--test", "scenarios", "adoption_"],
    ["cargo", "test", "--locked", "-p", "tl3-worktrees", "adoption_"],
    ["cargo", "test", "--locked", "-p", "tl3-github", "--test", "adoption"],
    *[["cargo", "test", "--locked", "-p", "tl3-controller", "--test", "publishing", "--", mode]
      for mode in ("adoption-pass", "adoption-fix", "adoption-extras", "adoption-negative")],
]
if args.proofs:
    commands.extend(["python3", "crates/kernel/run.py", "prove", name]
                    for name in ("apply_adoption_at_most_once", "apply_adoption_has_no_maker_credit"))
failed = []
for command in commands:
    print("RUN " + " ".join(command), flush=True)
    code = subprocess.run(command, check=False).returncode
    print(f"EXIT {code}: {' '.join(command)}", flush=True)
    if code:
        failed.append(command)
raise SystemExit(bool(failed))
