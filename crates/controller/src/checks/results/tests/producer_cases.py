"""Reproduce retained phases through run_tree -> parse rejection -> judge -> wire.

Run on BuildBuddy; --check verifies the committed Rust fixture, --write emits it.
Only process transport and progress publication are replaced, not producer decisions.
"""
import json
from pathlib import Path
import sys
import tempfile
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[7]
sys.path.insert(0, str(ROOT / 'harness/src'))
from tl3_harness import execution
from tl3_harness.model import Check, FailureNotes, Kind, Outcome, judge

TEST = 'tests/test_x.py::test_x'
RECORDS = [{'event': 'start', 'schema': 1}, {'event': 'collection', 'tests': [TEST]},
           *[{'event': 'phase', 'test': TEST, 'when': phase, 'outcome': 'passed',
              'assertion': False, 'missing': None} for phase in ['setup', 'call', 'teardown']],
           {'event': 'finish', 'exit_code': 1}]


class Process:
    exit_code = 1

    def __init__(self, *_):
        pass

    def __enter__(self):
        return self

    def __exit__(self, *_):
        return False

    def records(self):
        return ((json.dumps(record) + '\n').encode() for record in RECORDS)


class Progress:
    def commit(self, _):
        pass


def cases():
    with tempfile.TemporaryDirectory(dir=ROOT.parent) as directory:
        tree = Path(directory)
        (tree / 'tests').mkdir()
        (tree / 'tests/test_x.py').write_text('def test_x(): pass\n')
        image = execution.Container('sha256:' + '1' * 64)
        notes = FailureNotes()
        with patch.object(execution, 'Supervised', Process), patch.object(execution, 'container_command', return_value=[]):
            outcomes = execution.run_tree(tree, image, (TEST,), tree / 'report.jsonl', Progress(), messages=notes)
            assert outcomes == {TEST: Outcome.NO_REPORT}
            assert notes.phases[TEST]['call']['outcome'] == 'passed'
            # Pin the precise late rejection, after phases have been retained.
            try:
                execution.parse(RECORDS, (TEST,), set(), 1, messages=FailureNotes())
            except execution.InvalidReport as error:
                assert str(error) == 'pytest failed outside the named tests'
            else:
                raise AssertionError('the run must be rejected')
            check = Check('regression', Kind.REGRESSION, (TEST,))
            result = judge(check, {TEST: Outcome.PASSED}, outcomes, messages=notes)
            group = ('tests/test_x.py',)
            before = {group: execution.SelectedObservation({group[0]: Outcome.PASSED}, (TEST,))}
            # The real selected-head shell rewrites NoReport to Error but keeps phases.
            selected = execution.selected_head(type('Tool', (), {'run': staticmethod(execution.run_tree)})(),
                tree, image, (group,), before, tree, Progress(), 1)
        wire = json.loads(execution.result_json((result, selected)))
        assert wire[0]['tests'][0]['head'] == 'no-report'
        assert wire[1]['tests'][0]['head'] == 'error'
        assert wire[1]['tests'][0]['phases']['head']['call']['outcome'] == 'passed'
        return wire


if __name__ == '__main__':
    encoded = json.dumps(cases(), sort_keys=True, indent=2)
    source = '// Generated on BuildBuddy by producer_cases.py; verify with --check.\n'
    source += 'pub(super) const CASES: &str = r###"' + encoded + '"###;\n'
    fixture = Path(__file__).with_suffix('.rs')
    if sys.argv[1:] == ['--write']:
        fixture.write_text(source)
    else:
        assert sys.argv[1:] == ['--check']
        assert fixture.read_text() == source
    print('PRODUCER_PATH_PASS: run_tree/parse late rejection/judge/result_json and selected_head')
