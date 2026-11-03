"""Source proof survives replacement of an unsuitable audit-selected method."""
from copy import deepcopy
from importlib.resources import files

import pytest

from jsonschema import Draft202012Validator

from tl3_workers.contracts import schema
from tl3_workers.evidence import forcing_capabilities, forcing_guidance, preflight
from tl3_workers.roles import planner


CAPABILITY = {'tool': 'run_tests', 'executor': 'prismabuild_pytest',
              'runner_features': ['base_head'], 'artifacts': ['cas_receipt', 'terminal_verdict'],
              'methods': ['red_green', 'forced_reproduction.environment', 'forced_reproduction.patches'],
              'protected_files': []}


def obligation(method, *, source=False):
    return {**deepcopy(CAPABILITY), 'methods': [method],
            'requirement': 'Show the real old assertion fails and the corrected expectation passes.',
            'provenance': {'kind': 'source', 'source': 'issue.body',
                           'quote': 'Use helper_target_observer.'} if source else {'kind': 'method'}}


def test_t613_ac1_source_required_method_and_provenance_cannot_be_replaced():
    required = obligation('helper_target_observer', source=True)
    reply = {'kind': 'leaf', 'evidence_contract': [obligation('red_green')]}
    preflight({'tool_capabilities': [CAPABILITY], 'ingest': {'evidence_contract': [required]}},
              reply, planning=True)
    assert reply['kind'] == 'cant_meet'
    assert required in reply['evidence_contract']
    supported = {**deepcopy(CAPABILITY), 'methods': [*CAPABILITY['methods'], 'helper_target_observer']}
    alternative = {'kind': 'leaf', 'evidence_contract': [obligation('red_green')], 'checks': []}
    preflight({'tool_capabilities': [supported], 'ingest': {'evidence_contract': [required]}},
              alternative, planning=True)
    assert alternative['kind'] == 'cant_meet'
    assert 'Explicit source method helper_target_observer' in alternative['report']
    Draft202012Validator(schema('ingest-reply')['$defs']['evidence_requirement']).validate(required)


def test_t613_ac2_three_stage_reconciliation_replaces_methods_but_keeps_source():
    source = obligation('red_green', source=True)
    source['provenance']['quote'] = 'Show assertion RED on base and GREEN on head.'
    old = obligation('helper_target_observer')
    inputs = {'tool_capabilities': [CAPABILITY]}
    ingest = {'kind': 'brief', 'evidence_contract': [source, old]}
    preflight(inputs, ingest)
    audit = {'verdict': 'agree', 'objections': [], 'evidence_contract': [old]}
    preflight({**inputs, 'ingest': ingest}, audit)
    reply = {'kind': 'leaf', 'evidence_contract': [obligation('red_green')]}
    preflight({**inputs, 'ingest': ingest, 'audit_evidence_contract': audit['evidence_contract']},
              reply, planning=True)
    assert reply['kind'] == 'leaf', reply
    assert source in reply['evidence_contract']
    assert old not in reply['evidence_contract']
    assert reply['evidence_errors'] == []


@pytest.mark.parametrize('body', ["return {'errno': 5}", 'assert False'])
@pytest.mark.parametrize('decorated', [False, True])
def test_t613_ac4_helper_capability_does_not_establish_target_suitability(tmp_path, monkeypatch, body, decorated):
    (tmp_path / 'tests').mkdir()
    (tmp_path / 'pyproject.toml').write_text('[tool.tl3.harness]\ntest-files=["tests/*.py"]\n')
    (tmp_path / 'tests/test_evidence.py').write_text(
        ('@anything\n' if decorated else '') + f'def _eio():\n    {body}\n')
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(planner.Forecaster, 'measure', lambda *args: {'fits': True})
    inputs = {'must_split': False, 'protected_paths': [], 'ingest': {'brief': 'Correct evidence.',
              'goals': [{'description': 'Fix evidence.', 'verification': 'test'}]},
              'forecast': {'fit_tokens': 100000, 'context_tokens': 150000, 'overhead_tokens': {}}}
    reply = {'kind': 'leaf', 'files': ['tests/test_evidence.py'], 'edits_tests': [],
             'no_design_questions': True, 'lands_on_its_own': True,
             'checks': [{'kind': 'demonstrating', 'node_ids': ['tests/test_evidence.py::test_observer'],
                         'goal_ids': [0], 'helper_evidence': {'target': 'tests/test_evidence.py::_eio',
                                                          'failure_cases': [{'args': [], 'kwargs': {}}]}}]}
    planner.check_reply(inputs, reply)
    if decorated or body.startswith('return'):
        assert any('helper target' in v and ('undecorated' in v or 'assertion' in v)
                   for v in reply['violations']), reply
    else:
        assert reply['violations'] == []


def test_t613_ac5_path_depth_capability_is_explicitly_missing_and_names_stay_reserved():
    from tl3_harness import Forcing, HarnessInputError
    capabilities = forcing_capabilities()
    assert capabilities.get('path_depth', {}).get('supported') is False
    for name in ['TMPDIR', 'TEMP', 'TMP', 'PYTEST_DEBUG_TEMPROOT']:
        with pytest.raises(HarnessInputError):
            Forcing.parse({'symptom': 'Deep root.', 'environment': {name: '/tmp/deep'}})
    guidance = forcing_guidance()
    assert 'sandbox-local' in guidance and 'path-depth' in guidance


@pytest.mark.parametrize('role', ['ingest', 'audit', 'planner'])
def test_t613_ac6_refusal_and_selection_guidance_preserves_source_obligations(role):
    prompt = ' '.join(files('tl3_workers').joinpath('prompts', role + '.md').read_text().split())
    assert 'provenance' in prompt
    assert 'permitted environment forcings' in prompt
    assert 'remaining source obligation' in prompt
    assert 'Do not substitute review or a flaky-test forcing for required red/green proof.' not in prompt
    assert 'If that staging would copy the helper fix onto base and erase RED, select helper_target_observer' not in prompt


def test_t613_missing_contract_has_a_safe_repair_diagnostic():
    from tl3_workers.tracing import failure_tags
    with pytest.raises(ValueError) as caught:
        preflight({'tool_capabilities': [CAPABILITY]}, {'kind': 'brief'})
    assert failure_tags(caught.value)['failure_message'] == (
        'Return evidence_contract with source requirements and separately marked method choices.')
