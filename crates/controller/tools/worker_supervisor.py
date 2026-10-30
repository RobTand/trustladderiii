"""Durable waitpid owner. Claims are atomic links, never file locks."""

import json
import os
from pathlib import Path
import subprocess
import sys
import uuid


def publish(path, value, *, exclusive=False):
    temporary = path.with_name(path.name + "." + uuid.uuid4().hex)
    with temporary.open("x") as output:
        json.dump(value, output)
        output.flush()
        os.fsync(output.fileno())
    try:
        if exclusive:
            os.link(temporary, path)
        else:
            os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
        return True
    except FileExistsError:
        return False
    finally:
        temporary.unlink(missing_ok=True)


def identity():
    stat = Path(f"/proc/{os.getpid()}/stat").read_text().rsplit(")", 1)[1].split()
    return {
        "pid": os.getpid(),
        "machine": Path("/etc/machine-id").read_text().strip(),
        "start": int(stat[19]),
        "boot": Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
    }


def main():
    root = Path(sys.argv[1])
    if len(sys.argv) == 3 and sys.argv[2] == "--worker":
        # The exec keeps this identity, including across the parent dying.
        if not publish(root / "process.json", identity(), exclusive=True):
            return
        command = json.loads((root / "command.json").read_text())
        os.execv(sys.executable, [sys.executable, *command])
    if not publish(root / "claim.json", identity(), exclusive=True):
        return
    print("claimed", flush=True)
    sys.stdout.close()
    with (root / "request.json").open("rb") as request, \
         (root / "stdout.jsonl").open("wb", buffering=0) as output, \
         (root / "stderr.log").open("ab", buffering=0) as errors:
        worker = subprocess.Popen(
            [sys.executable, str(root / "supervisor.py"), str(root), "--worker"],
            stdin=request, stdout=output, stderr=errors,
        )
        code = worker.wait()
        os.fsync(output.fileno())
        os.fsync(errors.fileno())
    publish(root / "exit.json", {"code": code})


if __name__ == "__main__":
    main()
