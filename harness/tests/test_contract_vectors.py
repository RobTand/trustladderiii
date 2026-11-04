"""#734: golden check reports shared with the controller's parser and the worker schemas.

Each vector is the `results` that `run_checks` and `result_json` produce, the code the
trusted action runs, for the check request beside it. The controller parses every
vector (`results.rs`), and the worker schemas validate its projections, so a shape
changed on one side alone turns one of those tests red.

After an intended shape change, regenerate from a checkout:
    TL3_WRITE_CONTRACT_VECTORS=1 python -m pytest harness/tests/test_contract_vectors.py
A failing run also prints each produced vector on a `TL3_CONTRACT_VECTOR` line.
"""
import json
import os
from pathlib import Path
import subprocess
import sys

import pytest

from tl3_harness import execution, test_tool
from tl3_harness.execution import result_json, run_checks
from tl3_harness.model import Check, Forcing, Kind, Proof


VECTORS = Path(__file__).resolve().parents[1] / 'test-vectors/contracts'
RUNNER = Path(execution.__file__).with_name('runner.py')
IMAGE = 'sha256:' + '1' * 64
CONFIG = '[tool.tl3.harness]\ncommand=["python", "-m", "pytest"]\ntest-files=["tests/*.py"]\n'
REGENERATE = 'TL3_WRITE_CONTRACT_VECTORS=1 python -m pytest harness/tests/test_contract_vectors.py'
# Fixed identities make the commit and tree hashes in each receipt reproducible.
GIT_IDENTITY = {
    'GIT_AUTHOR_NAME': 'Fixture', 'GIT_AUTHOR_EMAIL': 'fixture@example.invalid',
    'GIT_COMMITTER_NAME': 'Fixture', 'GIT_COMMITTER_EMAIL': 'fixture@example.invalid',
    'GIT_AUTHOR_DATE': '2026-11-04T00:00:00Z', 'GIT_COMMITTER_DATE': '2026-11-04T00:00:00Z',
}

PRODUCT = 'import os\n\ndef value():\n    return {value}\n\ndef target():\n    return 1\n'
OTHER = 'def test_other():\n    assert 1 + 1 == 2\n'
# Assertions bind the value first: pytest's rewritten message would otherwise show a function address.
MESSAGE = ('import product\n\ndef test_value():\n    value = product.value()\n'
           '    assert value == 1, "value too small"\n')
SOURCE_ASSERTION = 'assert holds["transition_held_max_s"] >= SLOW_S'
FORCED_VALUE = '0 if os.getenv("REPRODUCE") else 1'
FORCED = 'import product\n\ndef test_value():\n    product.target()\n    value = product.value()\n    assert value == 1, "value too small"\n'
SYMPTOM = ('import os\n\ndef test_original():\n    if os.getenv("REPRODUCE"):\n'
           '        raise TimeoutError("deadline passed")\n')
SYMPTOM_FIXED = 'def test_original():\n    assert True\n'
PROOF = {'subjects': ['product.py::value'], 'observer_support': [], 'red_message': 'value too small'}
# runner.py records the qualified class name unclipped; test_tool.py refuses it past 300
# characters only after keeping (and hashing) the record (#750 review).
LONG_EXCEPTION = 'Deadline' + 'X' * 320


def plan_check(name, kind, tests, **extra):
    return {'name': name, 'kind': kind, 'tests': list(tests), **extra}


SCENARIOS = {
    # A subject/observer receipt whose RED came from the explicit assertion message.
    'proof': dict(
        base={'product.py': PRODUCT.format(value=0), 'tests/test_value.py': MESSAGE,
              'tests/test_other.py': OTHER},
        head={'product.py': PRODUCT.format(value=1)},
        plan=[plan_check('value is one', 'demonstrating', ['tests/test_value.py::test_value'], proof=PROOF),
              plan_check('other stays green', 'regression', ['tests/test_other.py::test_other'])]),
    # #709/#713: RED matched against the assertion's source beyond the display limit.
    'red-message-source-assertion': dict(
        base={'product.py': PRODUCT.format(value=0),
              'tests/test_holds.py': ('import product\n\nSLOW_S = 1\n\ndef test_hold():\n'
                                      '    holds = {"transition_held_max_s": product.value()}\n'
                                      f'    padding = {"x" * 2000!r}\n    {SOURCE_ASSERTION}\n')},
        head={'product.py': PRODUCT.format(value=1)},
        plan=[plan_check('hold reaches the minimum', 'demonstrating', ['tests/test_holds.py::test_hold'],
                         proof={**PROOF, 'red_message': SOURCE_ASSERTION})]),
    # #393: a forced reproduction echoes its forcing and reports the forced head run.
    'forcing': dict(
        base={'product.py': PRODUCT.format(value=FORCED_VALUE), 'tests/test_value.py': FORCED},
        head={'product.py': PRODUCT.format(value=1)},
        plan=[plan_check('forced value is one', 'demonstrating', ['tests/test_value.py::test_value'],
                         forcing={'symptom': 'The value drops when REPRODUCE is set.',
                                  'environment': {'REPRODUCE': '1'}, 'patches': []},
                         proof=PROOF)]),
    # 6c681fb319: a delay patch adds the per-side delay budget to the receipt.
    'delay-budget': dict(
        base={'product.py': PRODUCT.format(value=0), 'tests/test_value.py': FORCED},
        head={'product.py': PRODUCT.format(value=1)},
        plan=[plan_check('slow target keeps the value', 'demonstrating', ['tests/test_value.py::test_value'],
                         forcing={'symptom': 'The target is slow.', 'environment': {},
                                  'patches': [{'target': 'product.target', 'delay_seconds': 0.01}]},
                         proof=PROOF)]),
    # A head failure: the maker's failed-check feedback carries its bounded message.
    'head-failure': dict(
        base={'product.py': PRODUCT.format(value=1), 'tests/test_value.py': MESSAGE},
        head={'product.py': PRODUCT.format(value=0)},
        plan=[plan_check('value stays one', 'regression', ['tests/test_value.py::test_value'])]),
    # #359/#591: existing tests importing the change run as one more check with a selection receipt.
    'selection': dict(
        base={'pkg/__init__.py': '', 'pkg/core.py': 'def value():\n    return 1\n',
              'tests/test_core.py': 'from pkg import core\n\ndef test_core():\n    assert core.value() == 1\n',
              'tests/test_other.py': OTHER},
        head={'pkg/core.py': 'def value():\n    return 1  # changed\n'},
        plan=[plan_check('other stays green', 'regression', ['tests/test_other.py::test_other'])],
        selected_tests=50),
    # #741: a one-test budget admits the first importing unit and records the saturated suffix.
    'selection-saturated': dict(
        base={'pkg/__init__.py': '', 'pkg/core.py': 'def value():\n    return 1\n',
              'tests/test_core.py': 'from pkg import core\n\ndef test_core():\n    assert core.value() == 1\n',
              'tests/test_more.py': 'from pkg import core\n\ndef test_more():\n    assert core.value() > 0\n',
              'tests/test_other.py': OTHER},
        head={'pkg/core.py': 'def value():\n    return 1  # changed\n'},
        plan=[plan_check('other stays green', 'regression', ['tests/test_other.py::test_other'])],
        selected_tests=1),
    # #750 review: the symptom record keeps an exception name longer than 300 characters.
    'symptom-long-exception': dict(
        base={'tests/test_long.py': (f'class {LONG_EXCEPTION}(Exception):\n    pass\n\n\n'
                                     f'def test_long():\n    raise {LONG_EXCEPTION}("deadline passed")\n')},
        head={'tests/test_long.py': 'def test_long():\n    assert True\n'},
        plan=[plan_check('long exception symptom', 'demonstrating', ['tests/test_long.py::test_long'],
                         declared_symptom={'node': 'tests/test_long.py::test_long',
                                           'exception': 'builtins.TimeoutError', 'phase': 'call',
                                           'repeat': 1, 'parallel': 0})]),
    # #592/#626: a declared symptom carries every run's records as evidence.
    'declared-symptom': dict(
        base={'tests/test_original.py': SYMPTOM},
        head={'tests/test_original.py': SYMPTOM_FIXED},
        plan=[plan_check('original symptom', 'demonstrating', ['tests/test_original.py::test_original'],
                         forcing={'symptom': 'The original call times out.',
                                  'environment': {'REPRODUCE': '1'}, 'patches': []},
                         declared_symptom={'node': 'tests/test_original.py::test_original',
                                           'exception': 'builtins.TimeoutError', 'phase': 'call',
                                           'repeat': 1, 'parallel': 0})]),
}


def decode(plan):
    """The trusted action's own request decoding (action.py)."""
    return tuple(Check(check['name'], Kind(check['kind']), tuple(check['tests']),
                       None if check.get('forcing') is None else Forcing.parse(check['forcing']),
                       None if check.get('proof') is None else Proof.parse(check['proof']),
                       check.get('declared_symptom'))
                 for check in plan)


def commit(repo, files, message):
    for name, text in files.items():
        (repo / name).parent.mkdir(parents=True, exist_ok=True)
        (repo / name).write_text(text)
    env = {'PATH': os.environ['PATH'], 'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': os.devnull, **GIT_IDENTITY}
    for args in (('add', '--all'), ('commit', '-qm', message)):
        subprocess.run(['git', '-C', str(repo), '-c', 'core.hooksPath=/dev/null', *args],
                       env=env, check=True, capture_output=True)
    return subprocess.run(['git', '-C', str(repo), 'rev-parse', 'HEAD'], env=env, check=True,
                          capture_output=True, text=True).stdout.strip()


@pytest.fixture
def produce(tmp_path, monkeypatch):
    command = lambda tree, container, tests: [sys.executable, '-I', '-B', str(RUNNER), '-', str(tree)]
    for module in (execution, test_tool):
        monkeypatch.setattr(module, 'require_admission', lambda: None)
        monkeypatch.setattr(module, 'container_command', command)
    monkeypatch.setattr(execution.Progress, 'commit', lambda *_: None)
    monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_PATH', str(tmp_path / 'progress.json'))
    monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_TOKEN', 'fixture')

    def run(name):
        scenario = SCENARIOS[name]
        repo = tmp_path / 'served'
        repo.mkdir()
        subprocess.run(['git', 'init', '-q', '--template=', '-b', 'main', str(repo)], check=True, capture_output=True)
        monkeypatch.setenv('PRISMABUILD_CONTAINER_OWNER', str(repo))
        base = commit(repo, {'pyproject.toml': CONFIG, **scenario['base']}, 'base')
        head = commit(repo, scenario['head'], 'head')
        request = {'effect_key': f'checks:{name}', 'base': base, 'head': head, 'reviewed': None,
                   'plan': scenario['plan'], 'image': IMAGE}
        if 'selected_tests' in scenario:
            request['selected_tests'] = scenario['selected_tests']
        results = run_checks(repo, base, head, decode(request['plan']), container=execution.Container(IMAGE),
                             selected_tests=request.get('selected_tests'))
        return {'fixture': f'harness/tests/test_contract_vectors.py::{name}', 'request': request,
                'results': json.loads(result_json(results))}

    return run


@pytest.mark.parametrize('name', sorted(SCENARIOS))
def test_committed_vector_matches_the_harness(produce, name):
    produced = produce(name)
    path = VECTORS / f'{name}.json'
    if os.environ.get('TL3_WRITE_CONTRACT_VECTORS') == '1':
        VECTORS.mkdir(exist_ok=True)
        path.write_text(json.dumps(produced, indent=1, sort_keys=True) + '\n')
    committed = json.loads(path.read_text()) if path.is_file() else None
    if committed != produced:
        print('TL3_CONTRACT_VECTOR ' + name + ' ' + json.dumps(produced, sort_keys=True, separators=(',', ':')))
    assert committed == produced, (
        f'{path.name} no longer matches what the harness reports. If the shape change is intended, '
        f'regenerate with: {REGENERATE}; the controller and worker contract tests then check it.')


def test_every_scenario_has_one_committed_vector():
    assert sorted(path.stem for path in VECTORS.glob('*.json')) == sorted(SCENARIOS), (
        f'Add or remove vectors with: {REGENERATE}')
