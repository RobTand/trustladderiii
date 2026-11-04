"""When a model that can't serve a step now may be tried again (Q53).

Only provider-announced reset times and typed error replies count, never error text."""
from email.utils import parsedate_to_datetime
import math


def _announced(response, now):
    """The reset a 429's headers announce, in Unix seconds, or None when they name none."""
    headers = response.headers
    try:
        if "retry-after" in headers:
            value = headers["retry-after"]
            try:
                return now + float(value)
            except ValueError:
                return parsedate_to_datetime(value).timestamp()
        if "x-ratelimit-reset" in headers:
            return float(headers["x-ratelimit-reset"])
    except (ValueError, TypeError, OverflowError):
        pass
    return None


def _limited(error):
    response = getattr(error, "response", None)
    return response if getattr(response, "status_code", None) == 429 else None


def reset_ms(error, now):
    if (response := _limited(error)) is None:
        return None
    reset = _announced(response, now)
    if reset is None or not math.isfinite(reset) or reset <= now or reset * 1000 >= 2**64:
        return None
    return math.ceil(reset * 1000)


def _stale(error, now):
    """A 429 whose announced reset has already passed, or is zero."""
    if (response := _limited(error)) is None:
        return False
    reset = _announced(response, now)
    return reset is not None and math.isfinite(reset) and reset <= now


# Q53 (#745): how long a model that can't be served stays off its ladders before
# a step tries it again.
PROBE_S = 15 * 60
# #759: CLIProxyAPI answers every model with a bare 503 while it is down or
# restarting, and a stale or zero announced reset says the quota is back. Both
# try the model again soon, so one restart can't take every ladder out for 15 minutes.
SHORT_PROBE_S = 2 * 60
# CLIProxyAPI answers a model it can't route with 400 and this code
# (sdk/api/handlers/handlers_routing.go); OpenAI answers 404.
UNROUTABLE = "model_not_found"


def ending_ms(error, now, role):
    """When the failed step's model may be tried again, or None for a failure.

    An announced quota reset ends at that reset. A typed reply saying the model
    can't be served now ends at a probe (#745, #759)."""
    for link in _causes(error):
        if (reset := reset_ms(link, now)) is not None:
            return reset
        if (probe := _probe_s(link, now, role)) is not None:
            return math.ceil((now + probe) * 1000)
    return None


def event(error, now, request):
    """The quota event that ends this failed run, or None for a failure.

    The controller charges the ending to the run's model. A postmortem's grade
    judges run on the case file's grade model, so their ending names it (#759)."""
    role = request["role"]
    reset = ending_ms(error, now, role)
    if reset is None:
        return None
    ending = {"event": "quota", "role": role, "reset_ms": reset}
    if role == "postmortem" and any(getattr(_judged(link), "stage", None) == "grade judge failure"
                                    for link in _causes(error)):
        ending["model"] = request["inputs"]["grade_model"]
    return ending


def _causes(error):
    chain = []
    while error is not None and len(chain) < 8 and all(error is not seen for seen in chain):
        chain.append(error)
        error = error.__cause__ or (None if error.__suppress_context__ else error.__context__)
    return chain


def _judged(error):
    """A postmortem judge's failure, which keeps only its typed cause."""
    from tl3_workers.judges import JudgeFailure
    from tl3_workers.tracing import WorkerStageError

    if isinstance(error, WorkerStageError) and isinstance(error.judge_failure, JudgeFailure):
        return error
    return None


def _probe_s(error, now, role):
    from openai import APIStatusError

    if role == "postmortem" and (judged := _judged(error)) is not None:
        status, code = judged.judge_failure.status, judged.judge_failure.code
    elif isinstance(error, APIStatusError):
        status, code = error.status_code, error.code
    else:
        return None
    if status in (400, 404):
        # Every other 400 is the request's own fault and still fails.
        return PROBE_S if code == UNROUTABLE else None
    if status == 503:
        return SHORT_PROBE_S
    if status == 429:
        # Without an announced reset, the proxy's retries ran out.
        return SHORT_PROBE_S if _stale(error, now) else PROBE_S
    return None
