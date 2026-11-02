"""Run the current tree for the worker's test tool, inside an admitted action."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile

from .execution import Container, Progress, container_command, require_admission
from .report import decode
from .limits import MAX_REPORT_LINE_BYTES


SUMMARY_BYTES = 16_384
MESSAGE_CHARS = 1_000
PREFIX = "TL3_TEST_SUMMARY "
OUTCOMES = ("passed", "failed", "error", "skipped")
# A repeated test reports its worst outcome across runs.
WORST = ("passed", "skipped", "failed", "error")
RUN_FAILURES = 20


def options(repeat, parallel):
    """#408's bounds: fresh pytest processes, and xdist workers per process."""
    if type(repeat) is not int or not 1 <= repeat <= 10:
        raise ValueError("Use repeat 1 to 10 fresh pytest runs")
    if type(parallel) is not int or parallel and not 2 <= parallel <= 4:
        raise ValueError("Use parallel 2 to 4 xdist workers, or 0 for one pytest process")
    return repeat, parallel


def infrastructure(message, origin=None):
    result = {"status": "infrastructure_error", "message": message, "tests": [],
              "totals": dict.fromkeys(OUTCOMES, 0), "note": ""}
    if origin is not None:
        # Q50: the tested code may have caused this, so it is not citable
        # evidence that the environment blocked the maker.
        result["origin"] = origin
    return result


class Summary:
    def __init__(self):
        self.result = {"status": "completed", "tests": [],
                       "totals": dict.fromkeys(OUTCOMES, 0), "note": ""}
        self.omitted = False

    def add(self, test, outcome, message="", truncated=False):
        self.result["totals"][outcome] += 1
        item = {"test": test, "outcome": outcome, "message": message[:MESSAGE_CHARS]}
        clipped = truncated or len(message) > MESSAGE_CHARS
        self.result["tests"].append(item)
        # Leave space for totals, the action key and the truncation notice.
        if len(json.dumps(self.result).encode()) > SUMMARY_BYTES - 1024:
            self.result["tests"].pop()
            self.omitted = True
        if clipped or self.omitted:
            self.result["note"] = "Truncated: test entries or failure messages were cut. Totals include all results."


def run_current(tree, container, tests, parallel=0):
    require_admission()
    summary = Summary()
    missing = {test for test in tests if not (tree / test.split("::", 1)[0]).exists()}
    if missing:
        for test in sorted(missing):
            summary.add(test, "error", f"Selector not found: {test}")
        return summary.result
    progress = Progress()
    flags = [f"--parallel={parallel}"] if parallel else []
    command = container_command(tree, container, tests) + flags + ["--summary"]
    started = finished = False
    unavailable = None
    seen = set()
    collection_error = False
    with tempfile.TemporaryFile() as selection:
        selection.write(json.dumps(tests).encode())
        selection.seek(0)
        with subprocess.Popen(command, stdin=selection, stdout=subprocess.PIPE,
                                  stderr=subprocess.DEVNULL) as child:
            try:
                while line := child.stdout.readline(MAX_REPORT_LINE_BYTES + 1):
                    if len(line) > MAX_REPORT_LINE_BYTES:
                        raise ValueError("oversized report")
                    record = decode(line)
                    if not started:
                        if record != {"event": "start", "schema": 1}:
                            raise ValueError("missing report header")
                        started = True
                    elif finished:
                        raise ValueError("records after finish")
                    elif record.get("event") == "result":
                        if (set(record) != {"event", "test", "outcome", "message", "truncated"}
                                or not isinstance(record["test"], str) or record["test"] in seen
                                or record["outcome"] not in OUTCOMES
                                or not isinstance(record["message"], str)
                                or type(record["truncated"]) is not bool):
                            raise ValueError("invalid test result")
                        seen.add(record["test"])
                        summary.add(record["test"], record["outcome"], record["message"], record["truncated"])
                        progress.commit(record["test"])
                    elif record.get("event") == "collection-error":
                        collection_error = True
                        summary.add(record["test"], "error", record["message"], record["truncated"])
                        progress.commit(record["test"])
                    elif (record.get("event") == "unavailable" and parallel and unavailable is None
                          and set(record) == {"event", "message"} and isinstance(record["message"], str)):
                        unavailable = record["message"][:MESSAGE_CHARS]
                    elif record.get("event") == "finish" and set(record) == {"event", "exit_code"}:
                        finished = True
                        reported_exit = record["exit_code"]
                    else:
                        raise ValueError("unknown report event")
                code = child.wait()
                if (not finished or type(reported_exit) is not int or code != reported_exit
                        or code not in (0, 1, 2, 4, 5)
                        or code == 2 and not collection_error
                        or code == 0 and (summary.result["totals"]["failed"] or summary.result["totals"]["error"])
                        or code == 1 and not (summary.result["totals"]["failed"] or summary.result["totals"]["error"])):
                    raise ValueError("incomplete or inconsistent report")
                if unavailable is not None and (code or seen or collection_error):
                    raise ValueError("unavailable run reported tests")
            except (ValueError, KeyError, TypeError):
                child.terminate()
                code = child.wait()
                # Docker exits 125 to 127 when the container itself cannot run.
                if not started and code in (125, 126, 127):
                    return infrastructure("The test container could not start; no test verdict is available.")
                return infrastructure("The test run ended without a complete report; the code under "
                                      "test may have stopped pytest. No test verdict is available.",
                                      origin="test_run")
        if unavailable is not None:
            return {"status": "not_available", "message": unavailable, "tests": [],
                    "totals": dict.fromkeys(OUTCOMES, 0), "note": ""}
        if code == 4:
            for test in tests:
                summary.add(test, "error", f"Selector not found: {test}")
        if code == 5:
            summary.result["note"] = "No tests were collected."
        return summary.result


def run(tree, container, tests, *, repeat=1, parallel=0):
    """Run the tests `repeat` times, each a fresh pytest process, optionally under xdist."""
    repeat, parallel = options(repeat, parallel)
    extra = {"parallel": parallel} if parallel else {}
    if repeat == 1:
        return {**run_current(tree, container, tests, parallel), **extra}
    runs, worst, notes = [], {}, []
    for number in range(1, repeat + 1):
        result = run_current(tree, container, tests, parallel)
        if result["status"] != "completed":
            # No verdict from this run, so none for the repeat.
            result["message"] = f"Run {number} of {repeat}: {result['message']}"
            return {**result, "repeat": repeat, **extra, "runs": runs}
        failed = [item["test"] for item in result["tests"] if item["outcome"] in ("failed", "error")]
        runs.append({"run": number, "status": "completed", "totals": result["totals"],
                     "failed": failed[:RUN_FAILURES]})
        for item in result["tests"]:
            if item["test"] not in worst or WORST.index(item["outcome"]) > WORST.index(worst[item["test"]]["outcome"]):
                worst[item["test"]] = item
        notes.append(result["note"])
    summary = Summary()
    summary.result.update(repeat=repeat, **extra, runs=runs)
    clipped = False
    # Keep the run list within half the summary; later runs give up failure ids first.
    for entry in reversed(runs):
        while entry["failed"] and len(json.dumps(runs).encode()) > SUMMARY_BYTES // 2:
            entry["failed"].pop()
            clipped = True
    for item in worst.values():
        summary.add(item["test"], item["outcome"], item["message"])
    note = summary.result["note"] or next((note for note in notes if note), "")
    if clipped or any(note.startswith("Truncated") for note in notes):
        note = "Truncated: test entries or failure messages were cut. Totals include all results."
    summary.result["note"] = note
    return summary.result


def main():
    # This file is copied with the trusted harness, outside the served tree.
    parser = argparse.ArgumentParser()
    parser.add_argument("image")
    parser.add_argument("selectors")
    parser.add_argument("--exec-scratch", action="store_true")
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--parallel", type=int, default=0)
    args = parser.parse_args()
    try:
        result = run(Path("checkout").resolve(), Container(args.image, exec_scratch=args.exec_scratch),
                     tuple(json.loads(args.selectors)), repeat=args.repeat, parallel=args.parallel)
    except (OSError, RuntimeError, ValueError):
        result = infrastructure("Test infrastructure failed before a complete report was available.")
    print(PREFIX + json.dumps(result), flush=True)


if __name__ == "__main__":
    main()
