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
            raise ValueError("duplicate validator key")
        document[key] = value
    return document


def _reject_constant(_constant):
    raise ValueError("nonstandard validator constant")


def run_check(command, document, *, name="reply"):
    """One process boundary and strict JSON reader for every Rust validator."""
    binary = os.environ.get("TL3_BINARY")
    if not binary:
        raise ReplyCheckInfrastructureError(f"{name} validator could not start")
    try:
        result = subprocess.run(
            [binary, *command],
            input=json.dumps(document, allow_nan=False),
            text=True, encoding="utf-8", capture_output=True, check=True,
            timeout=TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired:
        raise ReplyCheckInfrastructureError(f"{name} validator timed out") from None
    except subprocess.CalledProcessError:
        raise ReplyCheckInfrastructureError(f"{name} validator process failed") from None
    except OSError:
        raise ReplyCheckInfrastructureError(f"{name} validator could not start") from None
    except (UnicodeError, ValueError, TypeError):
        raise ReplyCheckInfrastructureError(f"{name} validator returned invalid output") from None
    try:
        return json.loads(result.stdout, object_pairs_hook=_unique_object,
                          parse_constant=_reject_constant)
    except (ValueError, TypeError):
        raise ReplyCheckInfrastructureError(f"{name} validator returned invalid output") from None


def check_reply(role, inputs, reply):
    document = run_check(["check-reply", role], {"inputs": inputs, "reply": reply})
    violations = document.get("violations") if isinstance(document, dict) else None
    if not isinstance(violations, list) or any(not isinstance(item, str) for item in violations):
        raise ReplyCheckInfrastructureError("reply validator returned invalid output")
    if violations:
        raise ReplyViolation(violations[0])
