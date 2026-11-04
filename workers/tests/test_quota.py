import math
from types import SimpleNamespace

import httpx
import openai
import pytest

from tl3_workers.quota import PROBE_S, ending_ms, reset_ms


def response(headers, status=429):
    return SimpleNamespace(response=SimpleNamespace(status_code=status, headers=headers))


def test_provider_reset_time_is_required():
    assert reset_ms(response({"retry-after": "5"}), 1000) == 1005000
    assert reset_ms(response({"x-ratelimit-reset": "1008"}), 1000) == 1008000
    for headers in [{}, {"retry-after": "NaN"}, {"retry-after": "-1"},
                    {"retry-after": "bad"}, {"x-ratelimit-reset": "999"}]:
        assert reset_ms(response(headers), 1000) is None
    assert reset_ms(response({"retry-after": "5"}, 500), 1000) is None
    assert reset_ms(RuntimeError("secret exception text"), 1000) is None


def provider_error(status, body, headers=None):
    """A status error as the openai client builds it from a CLIProxyAPI reply."""
    reply = httpx.Response(status, headers=headers or {},
                           request=httpx.Request("POST", "http://proxy.invalid/v1"))
    error = {400: openai.BadRequestError, 404: openai.NotFoundError,
             429: openai.RateLimitError}.get(status, openai.InternalServerError)
    return error("private provider text", response=reply, body=body)


NOW = 1000.5
PROBE = math.ceil((NOW + PROBE_S) * 1000)
UNROUTABLE = {"message": "unknown provider for model gpt-6.1-sol",
              "type": "invalid_request_error", "code": "model_not_found", "param": "model"}


def test_unroutable_model_ends_like_quota_until_its_probe():
    assert ending_ms(provider_error(400, UNROUTABLE), NOW, "maker") == PROBE
    assert ending_ms(provider_error(404, UNROUTABLE), NOW, "reviewer") == PROBE


@pytest.mark.parametrize("body", [
    {"message": "private", "type": "invalid_request_error", "code": "invalid_value",
     "param": "temperature"},
    {"message": "private", "type": "invalid_request_error", "code": None, "param": None},
    {"message": "unknown provider for model gpt-6.1-sol", "type": "invalid_request_error"},
    "unknown provider for model gpt-6.1-sol: model_not_found",
    None,
])
def test_request_caused_400_still_fails(body):
    assert ending_ms(provider_error(400, body), NOW, "maker") is None


def test_allowance_unverified_and_retry_limit_end_like_quota():
    assert ending_ms(provider_error(503, {"message": "private", "type": "server_error"}),
                     NOW, "planner") == PROBE
    assert ending_ms(provider_error(429, {"message": "private"}), NOW, "maker") == PROBE
    # An announced reset is still the quota's own reset.
    assert ending_ms(provider_error(429, None, {"retry-after": "5"}), NOW, "maker") == math.ceil(
        (NOW + 5) * 1000)


def test_other_failures_are_not_unavailability():
    for status in (401, 403, 409, 422, 500, 502):
        assert ending_ms(provider_error(status, UNROUTABLE if status != 409 else None), NOW,
                         "maker") is None
    assert ending_ms(RuntimeError("model_not_found"), NOW, "maker") is None


def test_a_wrapped_provider_error_is_found():
    try:
        try:
            raise provider_error(400, UNROUTABLE)
        except openai.BadRequestError as error:
            raise RuntimeError("graph step failed") from error
    except RuntimeError as wrapped:
        assert ending_ms(wrapped, NOW, "maker") == PROBE


def test_postmortem_unavailability_still_fails():
    # A postmortem run uses two models and the controller charges its request's model.
    assert ending_ms(provider_error(400, UNROUTABLE), NOW, "postmortem") is None
