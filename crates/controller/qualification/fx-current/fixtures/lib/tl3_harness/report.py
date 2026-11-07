import json

from .model import Failure, Outcome
from .forcing_ownership import valid_delay_budget
from .node_ids import selects
from .red_message import RedMessageMatch


class InvalidReport(ValueError):
    pass


def parse(records: list[dict], tests: tuple[str, ...], missing: set[str], exit_code: int,
          added_names: frozenset[tuple[str, str]] = frozenset(), *,
          messages: dict[str, str] | None = None) -> dict[str, Outcome]:
    if not records:
        return dict.fromkeys(tests, Outcome.NO_REPORT)
    if records[0] != {"event": "start", "schema": 1}:
        raise InvalidReport("missing report header")
    if records[-1] != {"event": "finish", "exit_code": exit_code} or exit_code not in (0, 1, 2, 4, 5):
        raise InvalidReport("incomplete report or inconsistent process exit")
    if (len(records) == 4 and exit_code == 4
            and set(records[1]) == {"event", "message"} and records[1]["event"] == "plan-error"
            and isinstance(records[1]["message"], str) and 0 < len(records[1]["message"]) <= 1000
            and records[2] == {"event": "collection", "tests": []}):
        if messages is not None:
            messages.update(dict.fromkeys(tests, records[1]["message"]))
        return dict.fromkeys(tests, Outcome.UNCOLLECTED)
    details = getattr(messages, "details", None)
    phases = {}
    facts = {}
    selected = None
    collection_errors = {}
    delay_budgets = {}

    def added(name):
        if name is None:
            return False
        candidates = [name] if isinstance(name, list) and len(name) == 2 and all(isinstance(p, str) for p in name) else name
        if (not isinstance(candidates, list) or not candidates
                or any(not isinstance(pair, list) or len(pair) != 2
                       or not all(isinstance(part, str) for part in pair) for pair in candidates)):
            raise InvalidReport("invalid missing name")
        return any(tuple(pair) in added_names for pair in candidates)
    for record in records[1:-1]:
        message = record.get("message", "")
        if not isinstance(message, str) or len(message) > 1000:
            raise InvalidReport("invalid failure message")
        exception = record.get("exception", "")
        if not isinstance(exception, str) or len(exception) > 300:
            raise InvalidReport("invalid failure exception")
        exception_type = record.get('exception_type', '')
        if not isinstance(exception_type, str) or len(exception_type) > 200:
            raise InvalidReport('invalid phase exception type')
        reason = record.get('reason', '')
        if not isinstance(reason, str) or len(reason) > 1000:
            raise InvalidReport('invalid assertion reason')
        ignored = ('message', 'exception', 'exception_type', 'reason') if record.get('event') == 'phase' else ('message', 'exception')
        record = {key: value for key, value in record.items() if key not in ignored}
        red_message_match = None
        if record.get('event') == 'phase':
            expected = getattr(messages, 'red_message', None)
            assertion_failure = record.get('when') == 'call' and record.get('outcome') == 'failed' and record.get('assertion') is True
            if 'red_message_match' in record:
                if expected is None or not assertion_failure:
                    raise InvalidReport('unexpected RED message match')
                try:
                    red_message_match = RedMessageMatch(record.pop('red_message_match'))
                except (ValueError, TypeError):
                    raise InvalidReport('invalid RED message match') from None
            elif expected is not None and assertion_failure:
                raise InvalidReport('missing RED message match')
        match record:
            case {'event': 'delay-budget', 'test': str(test), 'applied_seconds': _, 'exhausted': _} if len(record) == 4:
                budget = {key: record[key] for key in ('applied_seconds', 'exhausted')}
                if (selected is None or test not in selected or not valid_delay_budget(budget)
                        or test in delay_budgets):
                    raise InvalidReport('invalid delay budget')
                delay_budgets[test] = budget
                if hasattr(messages, 'delay_budget'):
                    messages.delay_budget[test] = budget
            case {"event": "collection", "tests": list(names)} if set(record) in (
                    {"event", "tests"}, {"event", "tests", "saturation"}):
                if 'saturation' in record:
                    value = record['saturation']
                    if (not isinstance(value, dict) or set(value) != {'unit', 'cases', 'remaining'}
                            or not isinstance(value['unit'], list) or not value['unit']
                            or any(not isinstance(member, str) or member not in tests for member in value['unit'])
                            or type(value['cases']) is not int or type(value['remaining']) is not int
                            or not 0 <= value['remaining'] < value['cases']):
                        raise InvalidReport("invalid selection saturation")
                if (selected is not None or not all(isinstance(name, str) for name in names)
                        or len(set(names)) != len(names) or any(not any(selects(test, name) for test in tests) for name in names)):
                    raise InvalidReport("invalid test population")
                selected = names
            case {"event": "collection-error"} if len(record) == 1:
                collection_errors[""] = (Outcome.ERROR, message, exception)
            case {"event": "collection-error", "test": str(file), "missing": name} if len(record) == 3:
                if file in collection_errors or not any(test.split("::")[0] == file for test in tests):
                    raise InvalidReport("invalid collection error file")
                collection_errors[file] = (Outcome.ADDED_NAME if added(name) else Outcome.ERROR, message, exception)
            case {"event": "phase", "test": str(test), "when": str(when),
                  "outcome": str(outcome), "assertion": bool(assertion), "missing": name} if len(record) == 6:
                if (selected is None or test not in selected or when not in ("setup", "call", "teardown")
                        or outcome not in ("passed", "failed", "skipped") or (test, when) in phases
                        or assertion and (when != "call" or outcome != "failed")):
                    raise InvalidReport("invalid test phase")
                missing_added = added(name)
                previous = [phase for phase in ("setup", "call", "teardown") if (test, phase) in phases]
                if (when == "setup" and previous or when == "call" and previous != ["setup"]
                        or when == "teardown" and previous not in (["setup"], ["setup", "call"])):
                    raise InvalidReport("test phases are out of order")
                phases[test, when] = (outcome, assertion, missing_added)
                facts.setdefault(test, {'test': test})[when] = {
                    'outcome': outcome, 'exception': exception_type, 'assertion': assertion}
                if messages is not None and outcome == "failed" and message:
                    messages[test] = (messages.get(test, "") + "\n" + message).strip()[:1000]
                if details is not None and outcome == "failed":
                    if test not in details or when in ('setup', 'teardown'):
                        details[test] = Failure(when, exception, message, reason, red_message_match)
            case _:
                raise InvalidReport("unknown report event")
    def case_outcome(test):
        setup, call, teardown = [phases.get((test, phase)) for phase in ("setup", "call", "teardown")]
        if setup is None or teardown is None:
            raise InvalidReport("test did not finish")
        if setup[0] == "failed" or teardown[0] == "failed":
            return Outcome.ERROR
        if any(phase and phase[0] == "skipped" for phase in (setup, call, teardown)):
            return Outcome.SKIPPED
        if call is None:
            raise InvalidReport("test has no call report")
        if call[0] == "failed":
            return (Outcome.ASSERTION if call[1] else Outcome.ADDED_NAME
                    if call[2] else Outcome.ERROR)
        return Outcome.PASSED

    # Validate every collected case, including cases after the first failure.
    cases = {test: case_outcome(test) for test in selected or ()}
    outcomes = {}
    for test in tests:
        if test in missing:
            outcomes[test] = Outcome.MISSING
        elif (error := collection_errors.get(test.split("::")[0], collection_errors.get(""))) is not None:
            outcomes[test] = error[0]
            if messages is not None and error[1]:
                messages[test] = error[1]
            if details is not None:
                details[test] = Failure("collection", error[2], error[1])
        elif selected is None:
            raise InvalidReport("missing collection report")
        else:
            matches = [name for name in selected if selects(test, name)]
            failing = next((name for name in matches if cases[name] != Outcome.PASSED), None)
            outcomes[test] = (cases[failing] if failing is not None else
                              Outcome.PASSED if matches else Outcome.UNCOLLECTED)
            if messages is not None and failing is not None and failing in messages:
                messages[test] = messages[failing]
            if details is not None and failing in details:
                details[test] = details[failing]
            if hasattr(messages, 'phases') and matches:
                messages.phases[test] = facts[failing or matches[0]]
    if exit_code != 0 and all(value == Outcome.PASSED for value in outcomes.values()):
        raise InvalidReport("pytest failed outside the named tests")
    if exit_code == 0 and any(value in (Outcome.ASSERTION, Outcome.ADDED_NAME, Outcome.ERROR) for value in outcomes.values()):
        raise InvalidReport("pytest exit hides a reported failure")
    return outcomes


def decode(line: bytes) -> dict:
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise InvalidReport("duplicate JSON key")
            result[key] = value
        return result
    try:
        record = json.loads(line, object_pairs_hook=unique)
        if not isinstance(record, dict):
            raise InvalidReport("report record is not an object")
        return record
    except (ValueError, UnicodeDecodeError) as error:
        raise InvalidReport("invalid JSON report") from error
