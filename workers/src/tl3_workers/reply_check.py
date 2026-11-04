"""Worker-loop transport to the controller release's Rust validators."""

import json
import os
import subprocess


def check_reply(role, inputs, reply):
    result = subprocess.run(
        [os.environ["TL3_BINARY"], "check-reply", role],
        input=json.dumps({"inputs": inputs, "reply": reply}),
        text=True, encoding="utf-8", capture_output=True, check=True,
    )
    violations = json.loads(result.stdout)["violations"]
    if violations:
        raise ValueError(violations[0])
