"""Run the current tree for the worker's test tool, inside an admitted action."""
import argparse
import json
from pathlib import Path
import tempfile

from .execution import Container, Progress, container_command, require_admission
from .report import decode
from .selector_identity import selector_identity
from .model import Forcing
from .forcing_ownership import valid_delay_budget
from .supervision import ExecutionFailure, ExecutionOutcome, Supervised


SUMMARY_BYTES = 16_384
MESSAGE_CHARS = 999
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


def _summary_limit(result):
    failure = ExecutionFailure(ExecutionOutcome.TRANSPORT_FAILURE, 'summary', 'identity_exceeds_summary_limit')
    diagnostic = infrastructure(failure.message(), origin=result.get('origin'))
    diagnostic.update(totals=result['totals'], execution=result.get('execution', failure.as_json()),
                      note='Truncated: summary identity exceeds the envelope limit. No proof is available.')
    for field in ('action_key', 'receipts'):
        if field in result:
            diagnostic[field] = result[field]
    return diagnostic


def _summaries(result):
    yield result
    for group in result.get('groups', ()):
        yield from _summaries(group)
    if 'base' in result:
        yield from _summaries(result['base'])


def _shrink(summaries):
    for summary in summaries:
        selected = summary.get('selectors')
        compact = selector_identity(selected) if isinstance(selected, list) else None
        if compact is not None and len(json.dumps(compact)) < len(json.dumps(selected)):
            summary['selectors'] = compact
            return summary
    # Preserve failure details ahead of passing entries across the whole envelope.
    for passing_only in (True, False):
        for summary in summaries:
            tests = summary.get('tests', ())
            index = next((i for i in reversed(range(len(tests)))
                          if not passing_only or tests[i]['outcome'] == 'passed'), None)
            if index is not None:
                tests.pop(index)
                return summary
    for summary in summaries:
        for run in reversed(summary.get('runs', ())):
            if run.get('failed'):
                run['failed'].pop()
                return summary
    return None


def bounded(result, *, limit=SUMMARY_BYTES - 2048):
    # Worker base runs combine separately bounded groups. Bound their display
    # entries too, without changing any group's proof, totals or receipts.
    summaries = list(_summaries(result))
    size = len(json.dumps(result).encode())
    if size > limit:
        # Account for notices once, before measuring each shrinking iteration.
        for summary in summaries:
            summary['note'] = 'Truncated: selectors were compacted or entries were cut. Totals include all results.'
        size = len(json.dumps(result).encode())
    while size > limit:
        if _shrink(summaries) is None:
            return _summary_limit(result)
        reduced = len(json.dumps(result).encode())
        # The encoded byte count is a decreasing natural number, including the
        # truncation note. Every iteration shrinks it or exits without proof.
        if reduced >= size:
            return _summary_limit(result)
        size = reduced
    return result


class Summary:
    def __init__(self):
        self.result = {"status": "completed", "tests": [],
                       "totals": dict.fromkeys(OUTCOMES, 0), "note": ""}
        self.omitted = False
        # Every test's outcome, including entries the size bound drops; repeats aggregate these.
        self.outcomes = {}

    def add(self, test, outcome, message="", truncated=False, target_hits=None, forcing_refusal=None,
            forcing_tampered=False, failures=None, delay_budget=None):
        self.result["totals"][outcome] += 1
        item = {"test": test, "outcome": outcome, "message": message[:MESSAGE_CHARS]}
        if target_hits is not None:
            item["target_hits"] = target_hits
        if forcing_refusal is not None:
            item["forcing_refusal"] = forcing_refusal
        if forcing_tampered:
            item['forcing_tampered'] = True
        if failures is not None:
            item['failures'] = failures
        if delay_budget is not None:
            item['delay_budget'] = delay_budget
        self.outcomes[test] = item
        clipped = truncated or len(message) > MESSAGE_CHARS
        self.result["tests"].append(item)
        # Leave space for totals, the action key and the truncation notice.
        if len(json.dumps(self.result).encode()) > SUMMARY_BYTES - 1024:
            self.result["tests"].pop()
            self.omitted = True
        if clipped or self.omitted:
            self.result["note"] = "Truncated: test entries or failure messages were cut. Totals include all results."


def _run_current(tree, container, tests, parallel=0, summary=None, *, forcing=None,
                 declared_symptom=None, records=None):
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
        request = tests
        if forcing is not None or declared_symptom is not None:
            request = {'tests': tests, 'forcing': forcing.as_json() if forcing else {}}
            if declared_symptom is not None:
                request['declared_symptom'] = declared_symptom
        selection.write(json.dumps(request).encode())
        selection.seek(0)
        with Supervised(command, selection, tests) as process:
            try:
                for line in process.records():
                    record = decode(line)
                    if records is not None:
                        records.append(record)
                    if not started:
                        if record != {"event": "start", "schema": 1}:
                            raise ValueError("missing report header")
                        started = True
                    elif finished:
                        raise ValueError("records after finish")
                    elif record.get("event") == "result":
                        expected = {"event", "test", "outcome", "message", "truncated"}
                        failures = record.get('failures')
                        if declared_symptom is not None:
                            expected.add('failures')
                            if (not isinstance(failures, list) or len(failures) > 3
                                    or any(not isinstance(f, dict) or set(f) != {'exception', 'phase', 'source'}
                                           or not isinstance(f['exception'], str) or len(f['exception']) > 300
                                           or f['phase'] not in ('call', 'setup', 'teardown')
                                           or type(f['source']) is not bool for f in failures)):
                                raise ValueError('invalid symptom report')
                        hits = record.get("target_hits")
                        if forcing is not None and forcing.patches:
                            expected.add("target_hits")
                            if (not isinstance(hits, dict) or any(type(count) is not int or count < 0
                                                                 for count in hits.values())
                                    or set(hits) - {target for target, mode, _ in forcing.patches
                                                    if mode == "delay_seconds"}):
                                raise ValueError("invalid target invocation counts")
                        budget = record.get('delay_budget')
                        if budget is not None:
                            expected.add('delay_budget')
                            if (not valid_delay_budget(budget) or forcing is None
                                    or not any(mode == 'delay_seconds' for _, mode, _ in forcing.patches)):
                                raise ValueError('invalid delay budget')
                        refusal = record.get('forcing_refusal')
                        if refusal is not None:
                            expected.add('forcing_refusal')
                            if (not isinstance(refusal, dict)
                                    or set(refusal) != {'phase', 'target', 'resolved_module', 'owner_category', 'reason'}
                                    or refusal['phase'] != 'forcing_validation'
                                    or forcing is None or refusal['target'] not in {p[0] for p in forcing.patches}
                                    or any(not isinstance(value, str) or len(value) > 4096 for value in refusal.values())):
                                raise ValueError('invalid forcing refusal')
                        tampered = record.get('forcing_tampered', False)
                        if 'forcing_tampered' in record:
                            expected.add('forcing_tampered')
                            if (tampered is not True or forcing is None or record['outcome'] != 'error'
                                    or hits != {} or not record['message'].startswith('forcing_tampered:')):
                                raise ValueError('invalid forcing tamper report')
                        if (set(record) != expected
                                or not isinstance(record["test"], str) or record["test"] in seen
                                or record["outcome"] not in OUTCOMES
                                or not isinstance(record["message"], str)
                                or type(record["truncated"]) is not bool):
                            raise ValueError("invalid test result")
                        seen.add(record["test"])
                        summary.add(record["test"], record["outcome"], record["message"], record["truncated"], hits, refusal,
                                    tampered, failures, budget)
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


def run_current(tree, container, tests, parallel=0, summary=None, *, forcing=None,
                declared_symptom=None, records=None):
    try:
        return bounded(_run_current(tree, container, tests, parallel, summary, forcing=forcing,
                                    declared_symptom=declared_symptom, records=records))
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


def run_symptom(tree, container, tests, declaration, *, forcing=None):
    from .model import validate_symptom
    from .symptoms import receipt
    validate_symptom(declaration, tests)
    runs = []
    for number in range(1, declaration['repeat'] + 1):
        records = []
        result = run_current(tree, container, tests, declaration['parallel'], forcing=forcing,
                             declared_symptom=declaration, records=records)
        runs.append(receipt(records, result, declaration, number))
        if runs[-1]['outcome'] == 'unavailable':
            break
    result['symptom_runs'] = runs
    return result


def main():
    # This file is copied with the trusted harness, outside the served tree.
    parser = argparse.ArgumentParser()
    parser.add_argument("image")
    parser.add_argument("selectors")
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--parallel", type=int, default=0)
    parser.add_argument("--forcing")
    parser.add_argument('--declared-symptom')
    # A per-call value that only makes the PrismaBuild action key unique, so repeats run fresh.
    parser.add_argument("--fresh")
    args = parser.parse_args()
    try:
        forcing = Forcing.parse(json.loads(args.forcing)) if args.forcing else None
        if args.declared_symptom:
            declaration = json.loads(args.declared_symptom)
            if (args.repeat, args.parallel) != (declaration['repeat'], declaration['parallel']):
                raise ValueError('symptom campaign settings changed')
            result = run_symptom(Path('checkout').resolve(), Container(args.image),
                                 tuple(json.loads(args.selectors)), declaration, forcing=forcing)
            result['declared_symptom'] = declaration
        else:
            result = run(Path("checkout").resolve(), Container(args.image),
                         tuple(json.loads(args.selectors)), repeat=args.repeat, parallel=args.parallel, forcing=forcing)
        if forcing is not None:
            result.update(forcing=forcing.as_json(), selectors=json.loads(args.selectors))
        if args.declared_symptom:
            result['selectors'] = json.loads(args.selectors)
            for receipt in result['symptom_runs']:
                print('TL3_SYMPTOM_RUN ' + json.dumps(receipt, sort_keys=True, separators=(',', ':')), flush=True)
                del receipt['records']
    except (OSError, RuntimeError, ValueError):
        result = infrastructure("Test infrastructure failed before a complete report was available.")
    if Path('proof.json').is_file():
        result['proof'] = json.loads(Path('proof.json').read_text())
    result = bounded(result)
    print(PREFIX + json.dumps(result), flush=True)


if __name__ == "__main__":
    main()
