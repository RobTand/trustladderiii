"""Replay Q46 against the kernel and recorded upstreams, inside an admitted PB action."""
import subprocess
import sys

commands = [
    ["cargo", "test", "--locked", "-p", "tl3-controller", "--test", "publishing", "--", "--q46-open"],
    ["cargo", "test", "--locked", "-p", "tl3-kernel", "--test", "scenarios", "publishing_conflict", "--", "--nocapture"],
    ["cargo", "test", "--locked", "-p", "tl3-controller", "--test", "reproducing_rebase", "publication_", "--", "--nocapture"],
    *[["cargo", "test", "--locked", "-p", "tl3-controller", "--test", "publishing", "--", mode]
      for mode in ["q46-checking", "q46-reviewing", "q46-enqueue", "q46-clean", "d5-clean", "d5-conflict"]],
]
failed = False
for command in commands:
    print("RUN", " ".join(command), flush=True)
    result = subprocess.run(command)
    print("EXIT", result.returncode, flush=True)
    failed |= result.returncode != 0
sys.exit(int(failed))
