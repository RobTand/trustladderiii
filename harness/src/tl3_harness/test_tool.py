"""Run the current tree for the worker's test tool, inside an admitted action."""
import argparse
import json
from pathlib import Path
import tempfile

from .execution import Container, Progress, container_command, require_admission
from .report import decode
from .model import Forcing
from .supervision import ExecutionFailure, ExecutionOutcome, Supervised


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


def bounded(result, *, limit=SUMMARY_BYTES - 2048):
    # Bound the complete envelope after metadata is attached. Keep identity and
    # retained entries exact; dropping a node makes the proof visibly incomplete.
    while len(json.dumps(result).encode()) > limit:
        if result['status'] != 'completed' and result.get('selectors'):
            result['selectors'].pop()
            result['selectors_omitted'] = result.get('selectors_omitted', 0) + 1
        elif result['tests']:
            passing = next((i for i in reversed(range(len(result['tests'])))
                            if result['tests'][i]['outcome'] == 'passed'), -1)
            result['tests'].pop(passing)
        elif any(run.get('failed') for run in result.get('runs', ())):
            next(run for run in reversed(result['runs']) if run.get('failed'))['failed'].pop()
        else:
            failure = ExecutionFailure(ExecutionOutcome.TRANSPORT_FAILURE, 'summary', 'identity_exceeds_summary_limit')
            diagnostic = infrastructure(failure.message(), origin=result.get('origin'))
            diagnostic.update(totals=result['totals'], execution=result.get('execution', failure.as_json()),
                              note='Truncated: summary identity exceeds the envelope limit. No proof is available.')
            for field in ('action_key', 'receipts'):
                if field in result:
                    diagnostic[field] = result[field]
            return diagnostic
        result['note'] = 'Truncated: selectors, test entries or run failure IDs were cut. Totals include all results.'
    return result


class Summary:
    def __init__(self):
        self.result = {"status": "completed", "tests": [],
                       "totals": dict.fromkeys(OUTCOMES, 0), "note": ""}
        self.omitted = False
        # Every test's outcome, including entries the size bound drops; repeats aggregate these.
        self.outcomes = {}

    def add(self, test, outcome, message="", truncated=False, target_hits=None, forcing_refusal=None):
        self.result["totals"][outcome] += 1
        item = {"test": test, "outcome": outcome, "message": message[:MESSAGE_CHARS]}
        if target_hits is not None:
            item["target_hits"] = target_hits
        if forcing_refusal is not None:
            item["forcing_refusal"] = forcing_refusal
        self.outcomes[test] = item
        clipped = truncated or len(message) > MESSAGE_CHARS
        self.result["tests"].append(item)
        # Leave space for totals, the action key and the truncation notice.
        if len(json.dumps(self.result).encode()) > SUMMARY_BYTES - 1024:
            self.result["tests"].pop()
            self.omitted = True
        if clipped or self.omitted:
            self.result["note"] = "Truncated: test entries or failure messages were cut. Totals include all results."


def _run_current(tree, container, tests, parallel=0, summary=None, *, forcing=None):
    require_admission()
    summary = Summary() if summary is None else summary
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
        selection.write(json.dumps(tests if forcing is None else
                                   {"tests": tests, "forcing": forcing.as_json()}).encode())
        selection.seek(0)
        with Supervised(command, selection, tests) as process:
            try:
                for line in process.records():
                    record = decode(line)
                    if not started:
                        if record != {"event": "start", "schema": 1}:
                            raise ValueError("missing report header")
                        started = True
                    elif finished:
                        raise ValueError("records after finish")
                    elif record.get("event") == "result":
                        expected = {"event", "test", "outcome", "message", "truncated"}
                        hits = record.get("target_hits")
                        if forcing is not None and forcing.patches:
                            expected.add("target_hits")
                            if (not isinstance(hits, dict) or any(type(count) is not int or count < 0
                                                                 for count in hits.values())
                                    or set(hits) - {target for target, mode, _ in forcing.patches
                                                    if mode == "delay_seconds"}):
                                raise ValueError("invalid target invocation counts")
                        refusal = record.get('forcing_refusal')
                        if refusal is not None:
                            expected.add('forcing_refusal')
                            if (not isinstance(refusal, dict)
                                    or set(refusal) != {'phase', 'target', 'resolved_module', 'owner_category', 'reason'}
                                    or refusal['phase'] != 'forcing_validation'
                                    or forcing is None or refusal['target'] not in {p[0] for p in forcing.patches}
                                    or any(not isinstance(value, str) or len(value) > 4096 for value in refusal.values())):
                                raise ValueError('invalid forcing refusal')
                        if (set(record) != expected
                                or not isinstance(record["test"], str) or record["test"] in seen
                                or record["outcome"] not in OUTCOMES
                                or not isinstance(record["message"], str)
                                or type(record["truncated"]) is not bool):
                            raise ValueError("invalid test result")
                        seen.add(record["test"])
                        summary.add(record["test"], record["outcome"], record["message"], record["truncated"], hits, refusal)
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
                code = process.exit_code
                if (not finished or type(reported_exit) is not int or code != reported_exit
                        or code not in (0, 1, 2, 4, 5)
                        or code == 2 and not collection_error
                        or code == 0 and (summary.result["totals"]["failed"] or summary.result["totals"]["error"])
                        or code == 1 and not (summary.result["totals"]["failed"] or summary.result["totals"]["error"])):
                    raise ValueError("incomplete or inconsistent report")
                if unavailable is not None and (code or seen or collection_error):
                    raise ValueError("unavailable run reported tests")
            except (ValueError, KeyError, TypeError):
                failure = process.incomplete()
                result = infrastructure(failure.message(), origin="test_run" if process.report_started else None)
                result.update(execution=failure.as_json(), selectors=list(tests))
                return result
        if unavailable is not None:
            return {"status": "not_available", "message": unavailable, "tests": [],
                    "totals": dict.fromkeys(OUTCOMES, 0), "note": ""}
        if code == 4:
            for test in tests:
                summary.add(test, "error", f"Selector not found: {test}")
        if code == 5:
            summary.result["note"] = "No tests were collected."
        return summary.result


def run_current(tree, container, tests, parallel=0, summary=None, *, forcing=None):
    try:
        return bounded(_run_current(tree, container, tests, parallel, summary, forcing=forcing))
    except ExecutionFailure as failure:
        if failure.outcome == ExecutionOutcome.TEST_TIMEOUT:
            # Discard partial passes: a run that cannot finish has no GREEN verdict.
            result = Summary()
            for test in tests or (failure.test,):
                result.add(test, "error", failure.message())
            result = result.result
            result["message"] = failure.message()
        else:
            result = infrastructure(failure.message(), origin=(
                "test_run" if failure.outcome == ExecutionOutcome.UNKNOWN_INCOMPLETE else None))
        result.update(execution=failure.as_json(), selectors=list(tests))
        return bounded(result)


def run(tree, container, tests, *, repeat=1, parallel=0, forcing=None):
    """Run the tests `repeat` times, each a fresh pytest process, optionally under xdist."""
    repeat, parallel = options(repeat, parallel)
    if forcing is not None and (repeat != 1 or parallel):
        raise ValueError("A forcing rehearsal runs once without xdist")
    extra = {"parallel": parallel} if parallel else {}
    if repeat == 1:
        return {**(run_current(tree, container, tests, parallel) if forcing is None else
                   run_current(tree, container, tests, parallel, forcing=forcing)), **extra}
    runs, worst, notes = [], {}, []
    for number in range(1, repeat + 1):
        outcomes = Summary()
        result = run_current(tree, container, tests, parallel, outcomes)
        if result["status"] != "completed" or result.get("execution", {}).get("outcome") == "test_timeout":
            # No verdict from this run, so none for the repeat.
            result["message"] = f"Run {number} of {repeat}: {result['message']}"
            return bounded({**result, "repeat": repeat, **extra, "runs": runs})
        failed = [item["test"] for item in outcomes.outcomes.values() if item["outcome"] in ("failed", "error")]
        runs.append({"run": number, "status": "completed", "totals": result["totals"],
                     "failed": failed[:RUN_FAILURES]})
        for item in outcomes.outcomes.values():
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
    parser.add_argument("--forcing")
    # A per-call value that only makes the PrismaBuild action key unique, so repeats run fresh.
    parser.add_argument("--fresh")
    args = parser.parse_args()
    try:
        forcing = Forcing.parse(json.loads(args.forcing)) if args.forcing else None
        result = run(Path("checkout").resolve(), Container(args.image, exec_scratch=args.exec_scratch),
                     tuple(json.loads(args.selectors)), repeat=args.repeat, parallel=args.parallel, forcing=forcing)
        if forcing is not None:
            result.update(forcing=forcing.as_json(), selectors=json.loads(args.selectors))
    except (OSError, RuntimeError, ValueError):
        result = infrastructure("Test infrastructure failed before a complete report was available.")
    result = bounded(result)
    print(PREFIX + json.dumps(result), flush=True)


if __name__ == "__main__":
    main()
