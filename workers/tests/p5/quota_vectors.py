"""Shared quota vectors from the unmodified Python reference at cdacce8f99."""
import json
from types import SimpleNamespace

import httpx
import openai

from quota_reference import ending_ms


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
                    link = {"response_status": status, "headers": headers,
                            "api_status": status, "api_code": "model_not_found",
                            "judge": False, "judge_status": None, "judge_code": None,
                            "grade_judge": False}
                    facts = {"now": str(now), "role": "maker", "links": [link]}
                    vectors.append({"facts": facts, "expected": ending_ms(error, now, "maker")})
    return vectors


if __name__ == "__main__":
    print(json.dumps(rows(), allow_nan=False))
