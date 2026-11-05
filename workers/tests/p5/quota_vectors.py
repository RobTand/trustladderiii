"""Shared quota vectors from the unmodified Python reference at cdacce8f99."""
import json
from pathlib import Path
import sys
from types import SimpleNamespace

import httpx
import openai

from quota_reference import ending_ms, event
from tl3_workers.quota import facts
from tl3_workers.judges import FailureClass, JudgeFailure
from tl3_workers.tracing import WorkerStageError


def rows():
    vectors = []
    values = [None, "5", "0", "-1", "NaN", "Infinity", "-Infinity", "bad", "1_0",
              "  +1.25e1  ", "1e309", "0.0000000001", "1000.125", "18446744073709552",
              "Thu, 01 Jan 1970 00:16:48 GMT", "Thursday, 01-Jan-70 00:16:48 GMT",
              "Thu Jan  1 00:16:48 1970", "Thu, 01 Jan 1970 01:16:48 +0100"]
    for now in [1000.0, 1000.125, 1000.25, 1000.5]:
        for status in [400, 404, 429, 503, 500]:
            for name in ["retry-after", "x-ratelimit-reset"]:
                for value in values:
                    headers = {} if value is None else {name: value}
                    response = httpx.Response(status, headers=headers,
                                              request=httpx.Request("POST", "http://proxy.invalid"))
                    error = openai.APIStatusError("private", response=response,
                                                  body={"code": "model_not_found"})
                    vectors.append({"facts": facts(error, now, "maker"), "expected": ending_ms(error, now, "maker")})
    def add(error, role="maker"):
        document = facts(error, 1000.0, role)
        request = {"role": role, "inputs": {"grade_model": "grade"}}
        old_event = event(error, 1000.0, request)
        vectors.append({"facts": document, "expected": ending_ms(error, 1000.0, role),
                        "grade": old_event is not None and "model" in old_event})
    for status in [400, 404, 429, 503, 401, 403, 409, 422, 500, 502]:
        for code in [None, "invalid_value", "model_not_found"]:
            reply = httpx.Response(status, request=httpx.Request("POST", "http://proxy.invalid"))
            error = openai.APIStatusError("private", response=reply, body={"code": code})
            add(error)
            for stage in ["grade", "postmortem"]:
                judged = WorkerStageError(f"{stage} judge failure", judge_failure=JudgeFailure(FailureClass.HTTP, status, code))
                add(judged, "postmortem")
                add(judged, "maker")
    for headers in [{"retry-after": "bad", "x-ratelimit-reset": "1008"},
                    {"retry-after": "0", "x-ratelimit-reset": "1008"},
                    {"x-ratelimit-reset": "1000.125"}]:
        add(SimpleNamespace(response=SimpleNamespace(status_code=429, headers=headers),
                            __cause__=None, __context__=None, __suppress_context__=False))
    for depth in range(1, 11):
        reply = httpx.Response(429, headers={"retry-after": "5"}, request=httpx.Request("POST", "http://proxy.invalid"))
        error = openai.APIStatusError("private", response=reply, body=None)
        for _ in range(depth):
            wrapper = RuntimeError("private wrapper")
            wrapper.__cause__ = error
            error = wrapper
        add(error)
    error = RuntimeError("cycle")
    error.__cause__ = error
    add(error)
    return vectors


if __name__ == "__main__":
    generated = json.dumps(rows(), allow_nan=False, separators=(",", ":")) + "\n"
    if sys.argv[1:] == ["--check"]:
        assert Path(__file__).with_name("quota-vectors.json").read_text() == generated
    else:
        print(generated, end="")
