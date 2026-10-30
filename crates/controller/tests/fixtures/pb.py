"""Recorded queue fixture. Never imports PB or launches a real submission."""
import hashlib
import json
import os
import socket
from pathlib import Path
import sys

root, recordings = map(Path, sys.argv[1:3])
args = sys.argv[3:]
with (root / "calls.jsonl").open("a") as stream:
    stream.write(json.dumps(args) + "\n")

def save(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value))

def finish(key, request):
    mode = (root / "mode").read_text()
    if mode == "pending":
        save(root / "ready" / f"{key}.json", {"schema": "prismaquant.prismabuild.pool_item.v1",
             "action_key": key, "published_unix": 100, "attempt_history": []})
        return
    state = mode if mode in ("failed", "withdrawn") else "done"
    original = next((recordings / state).glob("*.json"))
    old_key = original.stem
    text = original.read_text().replace(old_key, key)
    value = json.loads(text)
    history = value.get("withdrawn_attempt") or value
    for link in history.get("attempt_history", []):
        relative = link["outcome"]
        source = recordings / relative.replace(key, old_key)
        save(root / relative, json.loads(source.read_text().replace(old_key, key)))
    save(root / state / f"{key}.json", value)
    (root / "ready" / f"{key}.json").unlink(missing_ok=True)
    report = {"schema": "tl3.checks.v1", "effect_key": request["effect_key"],
              "base": request["base"], "head": request["head"], "results": []}
    for check in request["plan"]:
        report["results"].append({"name": check["name"], "kind": check["kind"],
            "custody": "in-process", "verdict": "pass", "tests": [
                {"test": test, "base": "assertion" if check["kind"] == "demonstrating" else "passed",
                 "head": "passed", "pre_existing": False, "passed": True} for test in check["tests"]]})
    if mode in ("check_fail", "infrastructure"):
        result = report["results"][0]
        result["verdict"] = "fail" if mode == "check_fail" else "infrastructure-failure"
        for test in result["tests"]:
            test["head"] = "assertion" if mode == "check_fail" else "no-report"
            test["passed"] = False
            test["message"] = (root / "failure_message").read_text() if (root / "failure_message").exists() else "assert 'goodbye' == 'hello'" if mode == "check_fail" else "No test report."
    if (root / "recorded_results.json").exists():
        report["results"] = json.loads((root / "recorded_results.json").read_text())
    if mode == "wrong_head":
        report["head"] = "a" * 40
    if mode == "bad_report":
        report = {"invalid": True}
    raw = "" if mode in ("no_report", "withdrawn") else "TL3_CHECKS " + json.dumps(report) + "\n"
    log = root / f"{key}.stdout"
    log.write_text(raw)
    save(root / f"{key}.ending", {"schema": "prismabuild.endings.v1", "complete": True,
        "endings": [{"action_key": key, "status": "executed" if state == "done" else state,
            "succeeded": state == "done", "exit_code": 0 if state == "done" else None,
            "result": None if state == "withdrawn" else {"verified": True, "receipt_sha256": "a" * 64 if state == "done" else None,
                       "stdout": {"path": str(log), "bytes": len(raw.encode()),
                                  "sha256": hashlib.sha256(raw.encode()).hexdigest()}}}]})

def gate(stage):
    if (root / "publish_stage").exists() and (root / "publish_stage").read_text() == stage:
        with socket.socket(socket.AF_UNIX) as connection:
            connection.connect(str(root / "publish.sock"))
            connection.sendall(f"{os.getpid()}\n".encode())
            if connection.recv(1) != b"!":
                raise SystemExit("publisher gate closed")

if "--seal-only" in args:
    checkout = Path(args[args.index("--cwd") + 1])
    raw = (checkout / "request.json").read_bytes()
    key = hashlib.sha256(raw).hexdigest()
    save(root / f"{key}.request", json.loads(raw))
    print(json.dumps({"schema": "prismabuild.sealed.v1", "action_key": key, "transport": "pool"}))
elif "--publish-sealed" in args or "--finish" in args:
    key = args[1]
    if "--publish-sealed" in args and (root / "fail_publish").exists():
        raise SystemExit(1)
    gate("before")
    request = json.loads((root / f"{key}.request").read_text())
    if not any((root / state / f"{key}.json").exists() for state in ("ready", "done", "failed", "withdrawn")):
        with (root / "publications").open("a") as stream:
            stream.write(key + "\n")
        finish(key, request)
    if "--finish" in args:
        finish(key, request)
    gate("after")
    print(json.dumps({"schema": "prismabuild.submission.v1", "action_key": key,
                      "transport": "pool", "status": "attached", "published_unix": 100}))
    if (root / "lost_response").exists():
        raise SystemExit(1)
elif "--withdraw" in args:
    key = args[args.index("--withdraw") + 1]
    (root / "mode").write_text("withdrawn")
    finish(key, json.loads((root / f"{key}.request").read_text()))
    print(json.dumps({"schema": "prismabuild.withdrawal.v1", "action_key": key,
                      "withdrawn": True, "state_before": "ready"}))
elif "--endings" in args:
    if (root / "garbled_ending").exists():
        print("temporary tool failure")
        raise SystemExit(1)
    key = args[-1]
    path = root / f"{key}.ending"
    if path.exists():
        ending = json.loads(path.read_text())
        gate("ending")
        if (root / "cleanup_pending").exists():
            ending["complete"] = False
        print(json.dumps(ending))
    else:
        print(json.dumps({"schema": "prismabuild.endings.v1", "complete": False,
                          "endings": [{"action_key": key, "status": "waiting", "succeeded": False,
                                       "exit_code": None, "result": None}]}))
else:
    raise SystemExit(2)
