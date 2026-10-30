import json

from .model import Outcome


class InvalidReport(ValueError):
    pass


def parse(records: list[dict], tests: tuple[str, ...], missing: set[str], exit_code: int,
          added_names: frozenset[tuple[str, str]] = frozenset()) -> dict[str, Outcome]:
    if not records:
        return dict.fromkeys(tests, Outcome.NO_REPORT)
    if records[0] != {"event": "start", "schema": 1}:
        raise InvalidReport("missing report header")
    if records[-1] != {"event": "finish", "exit_code": exit_code} or exit_code not in (0, 1, 2, 4, 5):
        raise InvalidReport("incomplete report or inconsistent process exit")
    phases = {}
    selected = None
    collection_error = False
    for record in records[1:-1]:
        match record:
            case {"event": "collection", "tests": list(names)} if set(record) == {"event", "tests"}:
                if (selected is not None or not all(isinstance(name, str) for name in names)
                        or len(set(names)) != len(names) or not set(names) <= set(tests)):
                    raise InvalidReport("invalid test population")
                selected = set(names)
            case {"event": "collection-error"} if len(record) == 1:
                collection_error = True
            case {"event": "phase", "test": str(test), "when": str(when),
                  "outcome": str(outcome), "assertion": bool(assertion), "missing": name} if len(record) == 6:
                if (selected is None or test not in selected or when not in ("setup", "call", "teardown")
                        or outcome not in ("passed", "failed", "skipped") or (test, when) in phases
                        or assertion and (when != "call" or outcome != "failed")):
                    raise InvalidReport("invalid test phase")
                if name is not None and (not isinstance(name, list) or len(name) != 2
                                         or not all(isinstance(part, str) for part in name)):
                    raise InvalidReport("invalid missing name")
                previous = [phase for phase in ("setup", "call", "teardown") if (test, phase) in phases]
                if (when == "setup" and previous or when == "call" and previous != ["setup"]
                        or when == "teardown" and previous not in (["setup"], ["setup", "call"])):
                    raise InvalidReport("test phases are out of order")
                phases[test, when] = (outcome, assertion, tuple(name) if name else None)
            case _:
                raise InvalidReport("unknown report event")
    outcomes = {}
    for test in tests:
        if test in missing:
            outcomes[test] = Outcome.MISSING
        elif collection_error:
            outcomes[test] = Outcome.ERROR
        elif selected is None:
            raise InvalidReport("missing collection report")
        elif test not in selected:
            outcomes[test] = Outcome.UNCOLLECTED
        else:
            setup, call, teardown = [phases.get((test, phase)) for phase in ("setup", "call", "teardown")]
            if setup is None or teardown is None:
                raise InvalidReport("test did not finish")
            if setup[0] == "failed" or teardown[0] == "failed":
                outcomes[test] = Outcome.ERROR
            elif any(phase and phase[0] == "skipped" for phase in (setup, call, teardown)):
                outcomes[test] = Outcome.SKIPPED
            elif call is None:
                raise InvalidReport("test has no call report")
            elif call[0] == "failed":
                outcomes[test] = (Outcome.ASSERTION if call[1] else Outcome.ADDED_NAME
                                  if call[2] in added_names else Outcome.ERROR)
            else:
                outcomes[test] = Outcome.PASSED
    if exit_code != 0 and all(value == Outcome.PASSED for value in outcomes.values()):
        raise InvalidReport("pytest failed outside the named tests")
    if exit_code == 0 and any(value in (Outcome.ASSERTION, Outcome.ERROR) for value in outcomes.values()):
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
