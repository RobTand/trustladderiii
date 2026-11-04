import math
from types import SimpleNamespace

import httpx
import openai
import pytest

from tl3_workers.judges import FailureClass, JudgeFailure
from tl3_workers.quota import PROBE_S, SHORT_PROBE_S, ending_ms, event, reset_ms
from tl3_workers.tracing import WorkerStageError


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


SHORT = math.ceil((NOW + SHORT_PROBE_S) * 1000)


def test_allowance_unverified_and_retry_limit_end_like_quota():
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


def test_a_bare_503_probes_soon():
    # #759: CLIProxyAPI down or restarting answers every ladder entry with 503.
    # A short probe keeps one restart from taking every model out for 15 minutes.
    assert 2 * 60 <= SHORT_PROBE_S <= 5 * 60
    assert ending_ms(provider_error(503, {"message": "private", "type": "server_error"}),
                     NOW, "planner") == SHORT
    assert ending_ms(provider_error(503, None), NOW, "maker") == SHORT


@pytest.mark.parametrize("headers", [
    {"retry-after": "0"}, {"retry-after": "-1"}, {"x-ratelimit-reset": "999"},
    {"x-ratelimit-reset": "0"}, {"retry-after": "Thu, 01 Jan 1970 00:00:00 GMT"},
])
def test_a_stale_or_zero_announced_reset_retries_soon(headers):
    # #759: the provider says the quota is back already, so the step retries
    # soon instead of waiting out the 15-minute probe.
    assert reset_ms(response(headers), NOW) is None
    assert ending_ms(provider_error(429, None, headers), NOW, "maker") == SHORT


@pytest.mark.parametrize("headers", [{}, {"retry-after": "bad"}, {"retry-after": "NaN"}])
def test_a_429_without_a_usable_reset_keeps_the_probe(headers):
    assert ending_ms(provider_error(429, None, headers), NOW, "maker") == PROBE


POSTMORTEM = {"role": "postmortem", "model": {"name": "claude-opus-5-5"},
              "inputs": {"grade_model": "claude-sonnet-5-5"}}


def judge_error(stage, category, status=None, code=None):
    """A postmortem judge failure as roles/postmortem.py raises it: its chain is cut."""
    try:
        raise WorkerStageError(f"{stage} judge failure",
                               judge_failure=JudgeFailure(category, status, code)) from None
    except WorkerStageError as error:
        return error


@pytest.mark.parametrize("failure,expected", [
    ((FailureClass.HTTP, 400, "model_not_found"), PROBE),
    ((FailureClass.HTTP, 404, "model_not_found"), PROBE),
    ((FailureClass.HTTP, 503), SHORT),
    ((FailureClass.QUOTA, 429), PROBE),
])
def test_an_unavailable_postmortem_model_ends_like_quota(failure, expected):
    # #759: postmortems fall back like other roles. The writer's failure
    # charges the run's model, and a grade judge's charges the grade model.
    writer = judge_error("postmortem", *failure)
    assert ending_ms(writer, NOW, "postmortem") == expected
    assert event(writer, NOW, POSTMORTEM) == {
        "event": "quota", "role": "postmortem", "reset_ms": expected}
    grade = judge_error("grade", *failure)
    assert event(grade, NOW, POSTMORTEM) == {
        "event": "quota", "role": "postmortem", "reset_ms": expected,
        "model": "claude-sonnet-5-5"}


@pytest.mark.parametrize("failure", [
    (FailureClass.HTTP, 400), (FailureClass.HTTP, 400, "invalid_value"),
    (FailureClass.HTTP, 502), (FailureClass.REFUSAL, 403), (FailureClass.SCHEMA,),
    (FailureClass.DEADLINE,),
])
def test_other_postmortem_failures_still_fail(failure):
    for stage in ("grade", "postmortem"):
        assert event(judge_error(stage, *failure), NOW, POSTMORTEM) is None


def test_other_roles_never_name_a_model():
    error = provider_error(400, UNROUTABLE)
    request = {"role": "maker", "model": {"name": "m"}, "inputs": {}}
    assert event(error, NOW, request) == {"event": "quota", "role": "maker", "reset_ms": PROBE}
