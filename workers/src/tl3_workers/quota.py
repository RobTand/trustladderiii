"""Collect typed exception facts; Rust decides the Q53 reset and probe."""
from tl3_workers.reply_check import ReplyCheckInfrastructureError, run_check

# Public compatibility constants for callers and the Python reference vectors.
PROBE_S = 15 * 60
SHORT_PROBE_S = 2 * 60
UNROUTABLE = "model_not_found"


def _causes(error):
    chain = []
    while error is not None and len(chain) < 8 and all(error is not seen for seen in chain):
        chain.append(error)
        error = error.__cause__ or (None if error.__suppress_context__ else error.__context__)
    return chain


def _judged(error):
    from tl3_workers.judges import JudgeFailure
    from tl3_workers.tracing import WorkerStageError

    if isinstance(error, WorkerStageError) and isinstance(error.judge_failure, JudgeFailure):
        return error
    return None


def facts(error, now, role, *, chain=True):
    from openai import APIStatusError

    links = []
    for link in _causes(error) if chain else [error]:
        response = getattr(link, "response", None)
        headers = getattr(response, "headers", {})
        judged = _judged(link)
        failure = judged.judge_failure if judged is not None else None
        links.append({
            "response_status": getattr(response, "status_code", None),
            "headers": {name: headers[name] for name in ("retry-after", "x-ratelimit-reset")
                        if name in headers},
            "api_status": link.status_code if isinstance(link, APIStatusError) else None,
            "api_code": link.code if isinstance(link, APIStatusError) and isinstance(link.code, str) else None,
            "judge": judged is not None,
            "judge_status": failure.status if failure is not None else None,
            "judge_code": failure.code if failure is not None else None,
            "grade_judge": judged is not None and judged.stage == "grade judge failure",
        })
    return {"now": str(now), "role": role, "links": links}


def _decide(document, kind="ending"):
    result = run_check(["check-quota"], {"kind": kind, "facts": document}, name="quota")
    reset = result.get("reset_ms") if isinstance(result, dict) else False
    if reset is not None and (type(reset) is not int or reset < 0):
        raise ReplyCheckInfrastructureError("quota validator returned invalid output")
    if type(result.get("grade_judge")) is not bool:
        raise ReplyCheckInfrastructureError("quota validator returned invalid output")
    return result


def reset_ms(error, now):
    return _decide(facts(error, now, "", chain=False), "reset")["reset_ms"]


def ending_ms(error, now, role):
    return _decide(facts(error, now, role))["reset_ms"]


def event(error, now, request, *, raw=False):
    document = facts(error, now, request["role"])
    result = _decide(document)
    if result["reset_ms"] is None:
        return None
    ending = {"event": "quota", "role": request["role"]}
    ending.update({"facts": document} if raw else {"reset_ms": result["reset_ms"]})
    if result.get("grade_judge"):
        ending["model"] = request["inputs"]["grade_model"]
    return ending
