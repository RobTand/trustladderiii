"""Report budgets shared by planning, collection and the trusted reader."""
import json

MAX_REPORT_LINE_BYTES = 4 * 1024 * 1024
PLAN_SIZE_ERROR = f'Check plan exceeds the {MAX_REPORT_LINE_BYTES}-byte report limit; reduce the selected tests or parameter cases.'


def validate_plan_size(tests):
    tests = tuple(dict.fromkeys(tests))
    record = {'event': 'collection', 'tests': tests}
    size = len(json.dumps(record, sort_keys=True, ensure_ascii=False).encode()) + 1
    # A phase repeats one node ID and can include a 1,000-character JSON-escaped message.
    largest_phase = max((len(json.dumps(test, ensure_ascii=False).encode()) + 8192
                         for test in tests), default=0)
    if max(size, largest_phase) > MAX_REPORT_LINE_BYTES:
        raise ValueError(PLAN_SIZE_ERROR)
