"""Protocol v1 process boundary. stdout contains only protocol events."""

import argparse
from contextlib import redirect_stdout
import json
import os
from pathlib import Path
import sys

from tl3_workers.contracts import read_json, validate


def main():
    parser = argparse.ArgumentParser(prog="python -m tl3_workers")
    parser.add_argument("command", choices=["run"])
    parser.parse_args()
    output = sys.stdout

    def emit(event):
        output.write(json.dumps(event, ensure_ascii=False, allow_nan=False) + "\n")
        output.flush()

    phase = "request"
    try:
        with redirect_stdout(sys.stderr):
            request = read_json(sys.stdin.read())
            validate("request", request)
            validate(request["role"] + "-input", request["inputs"])
            if not Path(request["worktree"]).is_absolute():
                raise ValueError("worktree must be absolute")
            os.chdir(request["worktree"])
            phase = "run"
            from tl3_workers.runtime import run
            run(request, emit)
        return 0
    except Exception:
        # Provider errors can contain credentials or model text. Keep them off the wire.
        print(f"tl3_workers: {phase} failed", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
