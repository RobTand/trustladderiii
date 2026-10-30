#!/usr/bin/env python3
"""Run through PB on dl380g10 after deliberately applying model defaults."""
from pathlib import Path
import subprocess
import sys
import tomllib

here = Path(__file__).resolve().parent
home = Path.home()
config = home / ".config/tl3/tl3.toml"
before = tomllib.loads(config.read_text())
assert not (config.parent / "production-ready").exists(), "production gate must stay closed"
for attempt in range(2):
    subprocess.run(["bash", str(here / "prepare.sh"), *sys.argv[1:]], check=True)
    after = tomllib.loads(config.read_text())
    assert after["models"] == before["models"], "preparation clobbered operator model settings"
    assert after["prismabuild"] == before["prismabuild"], "preparation clobbered harness settings"
    assert not (config.parent / "production-ready").exists(), "production gate opened"
    print(f"Preparation {attempt + 1}: model names, efforts, budgets and harness image preserved; gate closed", flush=True)
subprocess.run([str(home / ".local/share/tl3/bin/tl3-deploy"), "validate", str(config)], check=True)
print("Full Config::load passed against deployed config", flush=True)
checked = subprocess.run([str(home / ".local/share/tl3/bin/tl3-deploy"), "check", str(config)])
assert checked.returncode == 1, "deployment must still report the deliberately closed gate"
print("tl3-deploy check exit 1: production startup and tunnel remain blocked", flush=True)
