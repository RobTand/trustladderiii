"""Scripted upstream replies shared by transport fixtures and real worker contracts."""
import json
import os
from pathlib import Path
import time

brief = {"kind": "brief", "brief": "Fix the greeting.", "goals": [
    {"description": "Return hello.", "verification": "test"}], "memory_ids": [], "files": ["greeting.py"]}


def publish_marker(path, text):
    pending = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    pending.write_text(text)
    pending.replace(path)


def scripted_reply(request, mode, marker):
    with marker.with_suffix(".requests").open("a") as output:
        output.write(json.dumps({"pid": os.getpid(), "request": request}) + "\n")
    if mode != "plan-blocked" or request["role"] == "planner":
        publish_marker(marker, str(os.getpid()))
    if mode == "quota-once" and not marker.with_suffix(".quota").exists():
        import httpx
        from openai import RateLimitError
        marker.with_suffix(".quota").write_text("announced")
        response = httpx.Response(429, headers={"retry-after": "600"}, request=httpx.Request("POST", "http://model.invalid/v1"))
        raise RateLimitError("private provider text", response=response, body=None)
    if mode == "blocked" or (mode in {"audit-blocked", "steering-commands", "steering-replacement"} and request["role"] == "audit") or (mode == "plan-blocked" and request["role"] == "planner"):
        while not marker.with_suffix(".release").exists():
            time.sleep(0.01)
    if request["role"] == "writer":
        inputs = request["inputs"]
        facts = inputs["facts"]
        if inputs["kind"] == "clarification":
            reply = {"text": 'About "' + facts["comment_excerpt"] + '", would you like me to stop or continue?'}
        else:
            reply = {key: facts[key] for key in ("obstacle", "options", "recommended_option",
                     "recommendation_reason", "how_to_answer")}
            reply["request_quote"] = facts["issue_title"]
        if not inputs.get("violations") or mode == "writer-always-multiline":
            if mode in {"writer-multiline", "writer-always-multiline"}:
                reply["text"] = reply["text"].replace(", would", ",\nwould")
            elif mode == "writer-wrong-quote":
                reply["request_quote"] = "A paraphrased request"
            elif mode == "writer-no-draft":
                reply = {"text": "Please choose an option."}
            elif mode == "writer-recommendation":
                reply["recommended_option"] = len(reply["options"])
            elif mode == "writer-no-excerpt":
                reply = {"text": "Would you like me to stop or continue?"}
            elif mode == "writer-no-text":
                reply = {"request_quote": facts["issue_title"], "obstacle": "I need your permission.",
                         "options": [{"action": "Continue", "outcome": "Work continues."},
                                     {"action": "Stop", "outcome": "Work stops."}],
                         "recommended_option": 0, "recommendation_reason": "This completes your request.",
                         "how_to_answer": "Reply with option one or two."}
        return reply
    if request["role"] == "classifier":
        body = request["inputs"]["comment"]["body"]
        replies = json.loads((Path(__file__).parent / "fixtures/steering/classifications.json").read_text())
        if body in {"Fail classification once", "Always fail classification", "Return an invalid classification"}:
            if body != "Fail classification once" or not marker.with_suffix(".failed-once").exists():
                publish_marker(marker.with_suffix(".failed-once"), "failed")
                if body == "Return an invalid classification":
                    return {"kind": "pause", "extra": True}
                raise ValueError("fixture classifier failure")
            reply = {"kind": "pause"}
        else:
            reply = replies[body]
        if reply.get("question_id") == "current":
            reply["question_id"] = request["inputs"]["state"]["open_question"]["id"]
        if body == "Answer the old question":
            publish_marker(marker.with_suffix(".classifying"), "ready")
            while not marker.with_suffix(".answer").exists():
                time.sleep(0.01)
        return reply
    if mode.startswith("steering") and request["role"] == "ingest" and request["operation"]["node"] != 0:
        while True:
            time.sleep(1)
    reply = brief if request["role"] == "ingest" else {"verdict": "agree", "objections": []}
    if mode == "steering-guidance" and request["role"] == "planner" and "Use the public API." in request["inputs"]["findings"]:
        publish_marker(marker.with_suffix(".guided"), "ready")
        while not marker.with_suffix(".release").exists():
            time.sleep(0.01)
    if request["role"] == "planner":
        reply = {"kind": "leaf", "files": ["greeting.py", "tests/test_greeting.py"],
                 "checks": [{"kind": "demonstrating", "node_ids": ["tests/test_greeting.py::test_hello"], "goal_ids": [0]}],
                 "no_design_questions": True, "lands_on_its_own": True}
        if mode == "plan-bad-schema":
            reply["checks"][0]["kind"] = "invalid"
        elif mode == "plan-invalid":
            reply["files"] = ["conftest.py"]
        elif mode in {"plan-split", "plan-bad-split"}:
            reply = {"kind": "split", "reason": "does_not_fit", "pieces": [
                {"brief": "Fix greeting.", "goal_ids": [0], "files": ["greeting.py"]},
                {"brief": "Test greeting.", "goal_ids": [0], "files": ["tests/test_greeting.py"]}]}
            if mode == "plan-bad-split": reply["pieces"] = reply["pieces"][:1]
        elif mode in {"plan-cant-meet", "steering-guidance"}:
            reply = {"kind": "cant_meet", "report": "Need a decision on the greeting.", "files": []}
    if mode == "early":
        reply = {"kind": "early_end", "outcome": "duplicate", "reason": "Already requested."} if request["role"] == "ingest" else reply
    if mode in {"disagree", "steering-answers"} and request["role"] == "audit":
        reply = {"verdict": "disagree", "objections": ["Preserve punctuation."]}
    return reply
