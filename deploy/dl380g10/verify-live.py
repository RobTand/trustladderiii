#!/usr/bin/env python3
"""Verify installed services through PB, including restarts before Rob's steps."""
import os
from pathlib import Path
import subprocess
import sys

os.environ.setdefault("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}")
home = Path.home()
check = [sys.executable, str(Path(__file__).with_name("deploy.py")), "check"]
required = {
    "OK tl3-postgres: active",
    "OK tl3-status: active",
    "OK Postgres reachable and migrated",
    "OK tl3 status",
    "OK LAN status HTTP",
    "OK operations issue configured",
    "OK sandbox installation configured",
    "OK harness image configured",
}


def inspect():
    result = subprocess.run(check, capture_output=True, text=True)
    print(result.stdout, end="", flush=True)
    print(f"deploy check exit: {result.returncode}", flush=True)
    if result.returncode not in (0, 1) or not required.issubset(result.stdout.splitlines()):
        raise SystemExit("available deployment services failed verification")


units = list((home / ".config/systemd/user").glob("tl3*.service"))
subprocess.run(["systemd-analyze", "--user", "--man=no", "verify", *map(str, units)], check=True)
inspect()
for services in (("tl3-status",), ("tl3-postgres", "tl3-status")):
    subprocess.run(["systemctl", "--user", "restart", *services], check=True)
    print(f"Restarted {' '.join(services)}", flush=True)
    inspect()
print("Available services survived restart. Missing checks above still block #19.")
