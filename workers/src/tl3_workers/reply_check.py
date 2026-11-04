"""Worker-loop transport to the controller release's Rust validators."""

import json
import os
import subprocess


TIMEOUT_SECONDS = 5


class ReplyViolation(ValueError):
    """A fixed Rust validator diagnostic, safe for repair and failure reporting."""


class ReplyCheckInfrastructureError(RuntimeError):
    """The validator did not decide; exit the worker without a model repair."""


def _unique_object(pairs):
    document = {}
    for key, value in pairs:
        if key in document:
            raise ReplyCheckInfrastructureError("reply validator returned invalid output")
        document[key] = value
    return document


def check_reply(role, inputs, reply):
    binary = os.environ.get("TL3_BINARY")
    if not binary:
        raise ReplyCheckInfrastructureError("reply validator could not start")
    try:
        result = subprocess.run(
            [binary, "check-reply", role],
            input=json.dumps({"inputs": inputs, "reply": reply}),
            text=True, encoding="utf-8", capture_output=True, check=True,
            timeout=TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired:
        raise ReplyCheckInfrastructureError("reply validator timed out") from None
    except subprocess.CalledProcessError:
        raise ReplyCheckInfrastructureError("reply validator process failed") from None
    except OSError:
        raise ReplyCheckInfrastructureError("reply validator could not start") from None
    except UnicodeError:
        raise ReplyCheckInfrastructureError("reply validator returned invalid output") from None
    try:
        document = json.loads(result.stdout, object_pairs_hook=_unique_object)
    except json.JSONDecodeError:
        raise ReplyCheckInfrastructureError("reply validator returned invalid output") from None
    violations = document.get("violations") if isinstance(document, dict) else None
    if not isinstance(violations, list) or any(not isinstance(item, str) for item in violations):
        raise ReplyCheckInfrastructureError("reply validator returned invalid output")
    if violations:
        raise ReplyViolation(violations[0])
