"""Run the current tree for the worker's test tool, inside an admitted action."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

from .execution import Container, Progress, container_command, require_admission
from .report import decode


SUMMARY_BYTES = 16_384
MESSAGE_CHARS = 1_000
PREFIX = "TL3_TEST_SUMMARY "
OUTCOMES = ("passed", "failed", "error", "skipped")


def infrastructure(message):
    return {"status": "infrastructure_error", "message": message, "tests": [],
            "totals": dict.fromkeys(OUTCOMES, 0), "note": ""}


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


def run_current(tree, container, tests):
    require_admission()
    summary = Summary()
    missing = {test for test in tests if not (tree / test.split("::", 1)[0]).exists()}
    if missing:
        for test in sorted(missing):
            summary.add(test, "error", f"Selector not found: {test}")
        return summary.result
    progress = Progress()
    command = container_command(tree, container, tests) + ["--summary"]
    started = finished = False
    seen = set()
    collection_error = False
    with tempfile.TemporaryFile() as selection:
        selection.write(json.dumps(tests).encode())
        selection.seek(0)
        with subprocess.Popen(command, stdin=selection, stdout=subprocess.PIPE,
                                  stderr=subprocess.DEVNULL) as child:
            try:
                while line := child.stdout.readline(65_537):
                    if len(line) > 65_536:
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
            except (ValueError, KeyError, TypeError):
                child.terminate()
                child.wait()
                return infrastructure("Container or pytest report failed; no test verdict is available.")
        if code == 4:
            for test in tests:
                summary.add(test, "error", f"Selector not found: {test}")
        if code == 5:
            summary.result["note"] = "No tests were collected."
        return summary.result


def main():
    # This file is copied with the trusted harness, outside the served tree.
    image, selectors = sys.argv[1:]
    try:
        result = run_current(Path("checkout").resolve(), Container(image), tuple(json.loads(selectors)))
    except (OSError, RuntimeError, ValueError):
        result = infrastructure("Test infrastructure failed before a complete report was available.")
    print(PREFIX + json.dumps(result), flush=True)


if __name__ == "__main__":
    main()
