"""Report budgets shared by planning, collection and the trusted reader."""
import json


MAX_REPORT_LINE_BYTES = 4 * 1024 * 1024
MAX_REPORT_BYTES = 32 * 1024 * 1024
MAX_RESULT_BYTES = 8 * 1024 * 1024
RESULT_ENVELOPE_BYTES = 65536
MAX_DETAILED_FAILURES = 20
MAX_MESSAGE_CHARS = 1000
PLAN_SIZE_ERROR = f'Check plan exceeds the {MAX_REPORT_LINE_BYTES}-byte report limit; reduce the selected tests or parameter cases.'
RESULT_SIZE_ERROR = f'Check plan exceeds the {MAX_RESULT_BYTES}-byte result limit; reduce the selected tests or checks.'


def validate_check_plan(plan):
    validate_plan_size(test for check in plan for test in check.tests)
    # JSON may escape a non-BMP message character as two six-byte surrogates.
    size = RESULT_ENVELOPE_BYTES + MAX_DETAILED_FAILURES * MAX_MESSAGE_CHARS * 12 + 2
    for check in plan:
        header = {'name': check.name, 'kind': check.kind, 'custody': 'in-process',
                  'verdict': 'infrastructure-failure', 'tests': []}
        size += len(json.dumps(header, separators=(',', ':'))) + 1
        for test in check.tests:
            case = {'test': test, 'base': 'uncollected', 'head': 'uncollected',
                    'pre_existing': False, 'passed': False, 'message': ''}
            size += len(json.dumps(case, separators=(',', ':'))) + 1
        if size > MAX_RESULT_BYTES:
            raise ValueError(RESULT_SIZE_ERROR)


def validate_plan_size(tests):
    tests = tuple(dict.fromkeys(tests))
    record = {'event': 'collection', 'tests': tests}
    size = len(json.dumps(record, sort_keys=True, ensure_ascii=False).encode(errors='backslashreplace')) + 1
    # A phase repeats one node ID and can include a 1,000-character JSON-escaped message.
    largest_phase = max((len(json.dumps(test, ensure_ascii=False).encode(errors='backslashreplace')) + 8192
                         for test in tests), default=0)
    if max(size, largest_phase) > MAX_REPORT_LINE_BYTES:
        raise ValueError(PLAN_SIZE_ERROR)
