# ruff: noqa: F811

from copy import deepcopy
from pathlib import Path
import sys
import pytest
from tl3_workers.forcing_rehearsal import ForcingRehearsals
from tl3_workers.roles import planner

from worker_process import IMAGE
from worker_process import NODE
from worker_process import git
from worker_process import plan
from worker_process import rehearsal  # noqa: F401


def test_inherited_repository_method_reaches_rehearsal(rehearsal):
    ledger, root, inputs, calls = rehearsal
    (root / 'publisher.py').write_text(
        'class Base:\n    def try_adopt(self):\n        return True\n'
        'class Publisher(Base):\n    pass\n')
    (root / 'tests/test_adoption.py').write_text(
        'from publisher import Publisher\ndef test_adopt():\n    assert Publisher().try_adopt()\n')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'inherited repository method')
    inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
    ledger = ForcingRehearsals(ledger.worktree, inputs)
    forcing = {'symptom': 'Inherited repository methods remain eligible.',
               'patches': [{'target': 'publisher.Publisher.try_adopt', 'returns': False}]}
    result = ledger.run([NODE], forcing)
    assert result['status'] == 'validated', result
    assert len(calls) == 2
    assert ledger.violations(inputs['base_sha'], plan(forcing)['checks']) == []


@pytest.mark.parametrize('binding', ['conditional_repository', 'dynamic_repository', 'dynamic_external',
                                     'class_global_repository'])
def test_uncertain_owner_uses_runtime_guard_and_preserves_typed_stop(rehearsal, monkeypatch, binding):
    ledger, root, inputs, calls = rehearsal
    (root / 'pkg').mkdir()
    (root / 'pkg/__init__.py').write_text('')
    source = ('class Clock:\n    @staticmethod\n    def monotonic():\n        return 1\n')
    source += {
        'conditional_repository': 'import time\nif True:\n    time = Clock\n',
        'dynamic_repository': 'time = (lambda: Clock)()\n',
        'dynamic_external': 'time = __import__("time")\n',
        # An executable class body rebinds the module global; source alone cannot settle it.
        'class_global_repository': 'import time\nclass Configure:\n    global time\n    time = Clock\n',
    }[binding]
    (root / 'pkg/mod.py').write_text(source)
    (root / 'tests/test_adoption.py').write_text(
        'from pkg.mod import time\ndef test_adopt():\n    assert time.monotonic() > 0\n')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'uncertain owner')
    inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
    ledger = ForcingRehearsals(ledger.worktree, inputs)
    target = 'pkg.mod.time.monotonic'
    forcing = {'symptom': 'The runtime guard decides uncertain ownership.',
               'patches': [{'target': target, 'returns': 0}]}
    result = ledger.run([NODE], forcing)
    assert len(calls) == 2, 'uncertain static owners must reach the admitted runtime guard'
    if binding == 'dynamic_external':
        # The observer imported the original shared module before forcing. A
        # private proxy on pkg.mod must leave that independent alias alone.
        assert result['status'] == 'refused', result
        assert 'ended passed; expected failed' in result['message']
    else:
        assert result['status'] == 'validated', result


def test_declared_symptom_rehearsal_binds_declaration_and_fixed_campaign(rehearsal):
    ledger, root, inputs, calls = rehearsal
    (root / 'tests/test_adoption.py').write_text('''
import os
from concurrent.futures import Future
def test_adopt():
    future = Future()
    if not os.getenv('DECLARED_TIMEOUT'):
        future.set_result(None)
    future.result(timeout=0)
''')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'original TimeoutError symptom')
    inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
    ledger = ForcingRehearsals(ledger.worktree, inputs)
    forcing = {'symptom': 'Future.result times out.', 'environment': {'DECLARED_TIMEOUT': '1'}, 'patches': []}
    symptom = {'node': NODE, 'exception': 'builtins.TimeoutError', 'phase': 'call', 'repeat': 2, 'parallel': 0}
    import inspect
    assert 'declared_symptom' in inspect.signature(ledger.run).parameters, 'rehearsal cannot declare exception RED'
    result = ledger.run([NODE], forcing, declared_symptom=symptom)
    assert result['status'] == 'validated', result
    checks = [{'kind': 'demonstrating', 'node_ids': [NODE], 'forcing': forcing, 'declared_symptom': symptom}]
    assert ledger.violations(inputs['base_sha'], checks) == []
    checks[0]['declared_symptom'] = {**symptom, 'repeat': 3}
    assert ledger.violations(inputs['base_sha'], checks), 'stale campaign receipt must be rejected'
    count = len(calls)
    assert ledger.run([NODE], forcing, declared_symptom=symptom) == result
    assert len(calls) == count, 'a declared campaign cannot retry until RED'


@pytest.mark.parametrize('mode', ['environment', 'patches'])
def test_t613_ac3_rehearsed_forcing_replaces_audit_helper_and_proves_real_assertion(rehearsal, monkeypatch, mode):
    from tl3_harness import execution, Forcing
    from tl3_harness.execution import Container, Progress, run_checks
    from tl3_harness.model import Check, Kind, Outcome, Verdict
    ledger, root, inputs, calls = rehearsal
    (root / 'publisher.py').write_text(
        'import os\ndef holder():\n    return os.environ.get("HOLDER_PATH", "tiny")\n')
    (root / 'pyproject.toml').write_text(
        '[tool.tl3.harness]\ncommand=["python", "-m", "pytest"]\ntest-files=["tests/*.py"]\n')
    old = '''import pytest
import publisher

def _eio():
    return {'errno': 5}

@pytest.mark.parametrize('limit', [4, 8])
def test_adopt(limit):
    pathname = publisher.holder()
    actual = pathname[:limit]
    assert actual == pathname
'''
    (root / 'tests/test_adoption.py').write_text(old)
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'parametrized old expectation')
    inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
    inputs['tool_capabilities'] = [{'tool': 'run_tests', 'executor': 'prismabuild_pytest',
        'runner_features': ['base_head'], 'artifacts': ['cas_receipt', 'terminal_verdict'],
        'methods': ['red_green', 'forced_reproduction.' + mode], 'protected_files': []}]
    chosen = {**inputs['tool_capabilities'][0], 'methods': ['helper_target_observer'],
              'requirement': 'Prove the bounded evidence expectation.', 'provenance': {'kind': 'method'}}
    from tl3_workers.evidence import preflight
    ingest = {**inputs['ingest'], 'evidence_contract': [deepcopy(chosen)]}
    preflight(inputs, ingest)
    audit = {'verdict': 'agree', 'objections': [], 'evidence_contract': [deepcopy(chosen)]}
    preflight({**inputs, 'ingest': ingest}, audit)
    inputs.update(ingest=ingest, audit_evidence_contract=audit['evidence_contract'])
    forcing = {'symptom': 'Holder pathname exceeds the bounded evidence length.',
               'environment': {'HOLDER_PATH': 'x' * 64} if mode == 'environment' else {},
               'patches': [{'target': 'publisher.holder', 'returns': 'x' * 64}] if mode == 'patches' else []}
    ledger = ForcingRehearsals(ledger.worktree, inputs)
    receipt = ledger.run([NODE], forcing)
    assert receipt['status'] == 'validated', receipt
    assert len(calls) == 2
    assert all(t['outcome'] == 'failed' for t in receipt['forced']['tests'])
    reply = {**plan(forcing), 'evidence_contract': []}
    planner.check_reply(inputs, reply, rehearsals=ledger)
    assert reply['kind'] == 'leaf', reply
    assert reply['violations'] == []
    assert reply['evidence_contract'] == []
    (root / 'tests/test_adoption.py').write_text(old.replace('assert actual == pathname',
                                                         'assert actual == pathname[:limit]'))
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'correct bounded expectation')
    head = git(root, 'rev-parse', 'HEAD')
    monkeypatch.setattr(execution, 'require_admission', lambda: None)
    monkeypatch.setenv('PRISMABUILD_CONTAINER_OWNER', str(root))
    monkeypatch.setattr(Progress, 'commit', lambda *_: None)
    runner = Path(execution.__file__).with_name('runner.py')
    monkeypatch.setattr(execution, 'container_command',
        lambda tree, container, tests: [sys.executable, '-I', '-B', str(runner), '-', str(tree)])
    check = Check('bounded evidence', Kind.DEMONSTRATING, (NODE,), Forcing.parse(forcing))
    result, = run_checks(root, inputs['base_sha'], head, (check,), container=Container(IMAGE))
    assert result.verdict == Verdict.PASS, result
    assert all(t.base == Outcome.ASSERTION and t.head == Outcome.PASSED for t in result.tests)
    assert result.forcing is not None
