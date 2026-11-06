"""Protocol v1 process boundary. stdout contains only protocol events."""

import argparse
import json
import os
from pathlib import Path
import sys
import time

from tl3_workers.contracts import read_json
from tl3_workers.reply_check import check_inputs


def main():
    parser = argparse.ArgumentParser(prog="python -m tl3_workers")
    parser.add_argument("command", choices=["run"])
    parser.parse_args()
    # LangSmith tracing would send issue text and model output off the host.
    os.environ["LANGSMITH_TRACING"] = "false"
    os.environ["LANGCHAIN_TRACING_V2"] = "false"
    # Writes to fd 1 from C extensions, child processes or sys.__stdout__
    # would corrupt the protocol stream, so fd 1 becomes stderr.
    output = os.fdopen(os.dup(1), "wb")
    os.dup2(2, 1)
    sys.stdout = sys.stderr

    def emit(event):
        # Encoding first means a reply that isn't valid UTF-8 fails the run
        # instead of reaching the controller half-written.
        line = (json.dumps(event, ensure_ascii=False, allow_nan=False) + "\n").encode("utf-8")
        output.write(line)
        output.flush()

    phase = "request"
    try:
        request = read_json(sys.stdin.buffer.read().decode("utf-8"))
        check_inputs("request", request)
        if not Path(request["worktree"]).is_absolute():
            raise ValueError("worktree must be absolute")
        os.chdir(request["worktree"])
        phase = "run"
        from tl3_workers.runtime import run
        run(request, emit)
        return 0
    except Exception as error:
        from tl3_workers.quota import event
        from tl3_workers.reply_check import ReplyCheckInfrastructureError
        try:
            ending = event(error, time.time(), request, raw=True) if phase == "run" else None
        except ReplyCheckInfrastructureError:
            ending = None
        if ending is not None:
            emit(ending)
            return 0
        # Provider errors can contain credentials or model text. Keep them off the wire.
        print(f"tl3_workers: {phase} failed", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
