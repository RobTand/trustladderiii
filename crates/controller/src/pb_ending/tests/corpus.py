"""Generate migration cases from the frozen Python, in one interpreter."""
from copy import deepcopy
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
import tempfile

spec = importlib.util.spec_from_file_location("pb_tests_before", sys.argv[1])
old = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = old
spec.loader.exec_module(old)
cases = []


def add(name, request, function):
    try:
        result = function()
        expected = {"result": result, "error": None}
    except old.ExecutionFailure as error:
        expected = {"result": None, "error": {"kind": "transport", "message": error.termination_reason}}
    except ValueError as error:
        expected = {"result": None, "error": {"kind": "value", "message": str(error)}}
    cases.append({"name": name, "request": request, "expected": expected})


for value in [None, -11, -10, -1, 0, 1, True, False, "-10", 1.0, [], {}]:
    add(f"priority {value!r}", {"kind": "priority", "priority": value}, lambda: old._priority(value))
for value in [[], ["a.py", "a.py", "b.py::test_é"], None, "a.py", ["/a"], ["../a"],
              ["a/.git/b"], ["-x"], ["a\\b"], ["a\nb"], [""], [True], ["a" * 2049], ["a"] * 257,
              ["./a//b.py::test"], ["a::test[../ok]"], ["a\u2028b"]]:
    add(f"selectors {value!r}", {"kind": "selectors", "tests": value}, lambda: old._selectors(value))
image = "sha256:" + "a" * 64
for field, values in {"image": ["mutable:tag", image, "répo/name@" + image], "tools_path": ["relative", "/tools"],
                      "cpus": [0, -1, 1, True, 1.0, "1"], "mem_gb": [0, 2], "quiet_seconds": [0, 120]}.items():
    for value in values:
        settings = dict(image=image, tools_path="/tools", cpus=1, mem_gb=2, quiet_seconds=120)
        settings[field] = value
        add(f"settings {field}={value!r}", {"kind": "settings", **settings},
            lambda: (old.TestSettings(**settings), None)[1])

with tempfile.TemporaryDirectory() as directory:
    path = Path(directory, "action.log")
    summary = {"status": "completed", "tests": [{"test": "a.py::test", "outcome": "passed", "message": "é😀"}],
               "totals": dict(passed=1, failed=0, error=0, skipped=0), "note": ""}
    raw = ("TL3_TEST_SUMMARY " + json.dumps(summary) + "\n").encode()
    ending = {"action_key": "a" * 64, "status": "executed", "succeeded": True, "exit_code": 0,
              "result": {"verified": True, "receipt_sha256": "b" * 64,
                         "stdout": {"path": str(path), "bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}}}

    def summary_case(name, value, log=raw):
        path.write_bytes(log)
        request = {"kind": "summary", "ending": value, "key": "a" * 64,
                   "raw": list(log), "length": len(log), "sha256": hashlib.sha256(log).hexdigest()}
        add(name, request, lambda: old._summary(deepcopy(value), "a" * 64))

    summary_case("valid", ending)
    for fields in [("action_key",), ("status",), ("succeeded",), ("exit_code",), ("result",),
                   ("result", "verified"), ("result", "receipt_sha256"), ("result", "stdout"),
                   ("result", "stdout", "bytes"), ("result", "stdout", "sha256")]:
        for bad in [None, False, True, -1, 1, "wrong", [], {}]:
            value = deepcopy(ending)
            parent = value
            for field in fields[:-1]:
                parent = parent[field]
            parent[fields[-1]] = bad
            summary_case(f"corrupt {fields} {bad!r}", value)
        value = deepcopy(ending)
        parent = value
        for field in fields[:-1]:
            parent = parent[field]
        del parent[fields[-1]]
        summary_case(f"missing {fields}", value)
    for field in ["status", "tests", "totals", "note"]:
        for bad in [None, False, -1, "wrong", [], {}]:
            result = {**summary, field: bad}
            log = ("TL3_TEST_SUMMARY " + json.dumps(result) + "\n").encode()
            value = deepcopy(ending)
            value["result"]["stdout"].update(bytes=len(log), sha256=hashlib.sha256(log).hexdigest())
            summary_case(f"summary {field} {bad!r}", value, log)
    for log in [b"", raw + raw, b"TL3_TEST_SUMMARY invalid\n", b"\xff"]:
        value = deepcopy(ending)
        value["result"]["stdout"].update(bytes=len(log), sha256=hashlib.sha256(log).hexdigest())
        summary_case(f"raw {log!r}", value, log)
print(json.dumps(cases, ensure_ascii=False))
