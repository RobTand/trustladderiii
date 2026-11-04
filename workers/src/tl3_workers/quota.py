"""When a model that can't serve a step now may be tried again (Q53).

Only provider-announced reset times and typed error replies count, never error text."""
from email.utils import parsedate_to_datetime
import math


def reset_ms(error, now):
    response = getattr(error, "response", None)
    if getattr(response, "status_code", None) != 429:
        return None
    headers = response.headers
    try:
        if "retry-after" in headers:
            value = headers["retry-after"]
            try:
                reset = now + float(value)
            except ValueError:
                reset = parsedate_to_datetime(value).timestamp()
        elif "x-ratelimit-reset" in headers:
            reset = float(headers["x-ratelimit-reset"])
        else:
            return None
        if not math.isfinite(reset) or reset <= now or reset * 1000 >= 2**64:
            return None
        return math.ceil(reset * 1000)
    except (ValueError, TypeError, OverflowError):
        return None


# Q53 (#745): how long a model that can't be served stays off its ladders before
# a step tries it again.
PROBE_S = 15 * 60
SHORT_PROBE_S = 2 * 60


def event(error, now, request):
    """The quota event that ends this failed run, or None for a failure."""
    reset = ending_ms(error, now, request["role"])
    if reset is None:
        return None
    return {"event": "quota", "role": request["role"], "reset_ms": reset}


def ending_ms(error, now, role):
    """When the failed step's model may be tried again, or None for a failure.

    An announced quota reset ends at that reset. A typed reply saying the model
    can't be served now ends at a probe, PROBE_S later (#745)."""
    # A postmortem calls two models, and the controller charges its ending to
    # the request's model, so only its own announced quota ends it.
    if role == "postmortem":
        return reset_ms(error, now)
    for link in _causes(error):
        if (reset := reset_ms(link, now)) is not None:
            return reset
        if _unavailable(link):
            return math.ceil((now + PROBE_S) * 1000)
    return None


def _causes(error):
    chain = []
    while error is not None and len(chain) < 8 and all(error is not seen for seen in chain):
        chain.append(error)
        error = error.__cause__ or (None if error.__suppress_context__ else error.__context__)
    return chain


def _unavailable(error):
    from openai import APIStatusError

    if not isinstance(error, APIStatusError):
        return False
    status = error.status_code
    if status in (400, 404):
        # CLIProxyAPI answers a model it can't route with 400 and this code
        # (sdk/api/handlers/handlers_routing.go); OpenAI answers 404. Every
        # other 400 is the request's own fault and still fails.
        return error.code == "model_not_found"
    # 503: the upstream can't serve the model now, such as an unverified
    # allowance. 429 without an announced reset: the proxy's retries ran out.
    return status in (429, 503)
