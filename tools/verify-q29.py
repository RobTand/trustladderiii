"""Replay Q29 config and gate scenarios inside an admitted PrismaBuild action."""
import subprocess
import sys

commands = [
    ["cargo", "test", "--locked", "-p", "tl3-config", "--test", "config", "--", "--nocapture"],
    ["cargo", "test", "--locked", "-p", "tl3-controller", "--test", "gate_slots", "--", "--nocapture"],
]
failed = False
for command in commands:
    print("RUN", " ".join(command), flush=True)
    result = subprocess.run(command)
    print("EXIT", result.returncode, flush=True)
    failed |= result.returncode != 0
sys.exit(int(failed))
