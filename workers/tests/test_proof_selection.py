"""Reconcile audit method choices through real bounded forcing and checking."""
from copy import deepcopy
from pathlib import Path
import sys

import pytest

from test_pb_tests import IMAGE, NODE, git, plan, rehearsal  # noqa: F401
from tl3_workers.forcing_rehearsal import ForcingRehearsals
from tl3_workers.roles import planner


@pytest.mark.parametrize('mode', ['environment', 'patches'])
def test_t613_ac3_rehearsed_forcing_replaces_audit_helper_and_proves_real_assertion(rehearsal, monkeypatch, mode):  # noqa: F811
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
