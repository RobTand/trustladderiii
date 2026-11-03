"""Planner forcing evidence contracts."""
# ruff: noqa: F811

import copy
from pathlib import Path

import pytest

from tl3_workers.evidence import RESERVED_ENVIRONMENT, RESERVED_PREFIXES
from tl3_workers.contracts import validate
import worker_process
from planner_fixtures import (
    DIGESTS,
    LEAF,
    cache_tokenizer,  # noqa: F401
    deflake_worktree,  # noqa: F401
    digests_plan,  # noqa: F401
    planned,
    planner_request,  # noqa: F401
    request_149_plan,  # noqa: F401
    run_reply,
    tool_capable_fake,  # noqa: F401
)


def test_forced_reproduction_needs_a_matching_rehearsal(planner_request, deflake_worktree):
    from tl3_workers.roles.planner import check_reply

    validate("planner-reply", deflake_worktree)
    check_reply(planner_request['inputs'], deflake_worktree)
    assert any('matching forcing rehearsal receipt' in v for v in deflake_worktree['violations'])


def test_forcing_belongs_only_on_demonstrating_checks(planner_request, deflake_worktree):
    from tl3_workers.roles.planner import check_reply

    deflake_worktree['checks'][0]['kind'] = 'regression'
    check_reply(planner_request['inputs'], deflake_worktree)
    assert "Only a demonstrating check can carry a forcing." in deflake_worktree['violations']


def test_forced_check_names_the_old_test_at_the_base(planner_request, deflake_worktree):
    from tl3_workers.roles.planner import check_reply

    test = 'tests/test_sampler.py::test_departure_is_stated'
    deflake_worktree['checks'][0]['node_ids'] = [test]
    check_reply(planner_request['inputs'], deflake_worktree)
    assert any(test in item and 'base' in item for item in deflake_worktree['violations'])


def test_invalid_forcing_is_a_planner_violation(planner_request, deflake_worktree):
    from tl3_workers.roles.planner import check_reply

    deflake_worktree['checks'][0]['forcing']['patches'][0]['raises'] = 'AssertionError'
    validate("planner-reply", deflake_worktree)
    check_reply(planner_request['inputs'], deflake_worktree)
    assert any(item.startswith('Invalid forcing:') and 'AssertionError' in item
               for item in deflake_worktree['violations'])


@pytest.mark.parametrize('change', [
    {'code': 'print(1)'},
    {'symptom': ''},
    {'patches': [{'target': 'sampler.f', 'exec': 'x'}]},
])
def test_planner_schema_keeps_the_forcing_declarative(deflake_worktree, change):
    import jsonschema

    deflake_worktree['checks'][0]['forcing'].update(change)
    with pytest.raises(jsonschema.ValidationError):
        validate("planner-reply", deflake_worktree)


def test_oversized_check_plan_is_a_planner_violation(planner_request, monkeypatch):
    from tl3_workers.roles.planner import check_reply
    monkeypatch.chdir(planner_request['worktree'])
    reply = copy.deepcopy(LEAF)
    reply['checks'] = [{'proof': {'subjects': [], 'observer_support': [], 'red_message': 'expected regression'}, 'kind': 'demonstrating', 'goal_ids': [0, 1],
                        'node_ids': ['tests/test_greeting.py::test_' + 'x' * (4 * 1024 * 1024)]}]
    check_reply(planner_request['inputs'], reply)
    assert any('4194304' in violation and 'reduce' in violation for violation in reply['violations'])


@pytest.mark.parametrize('kind', ['regression', 'behavior-preserving'])
def test_existing_test_in_a_large_file_is_defined_at_the_base(planner_request, monkeypatch, kind):
    # A test module over 1 MiB still defines its tests; only the 16 MiB cap applies.
    from tl3_workers.roles.planner import check_reply
    from tl3_workers.forecast import Forecaster

    root = Path(planner_request['worktree'])
    (root / 'tests').mkdir(exist_ok=True)
    padding = "".join(f"def helper_{i}():\n    return {i}\n\n\n" for i in range(45_000))
    (root / 'tests/test_large.py').write_text(padding + "def test_existing():\n    assert True\n")
    assert (root / 'tests/test_large.py').stat().st_size > 1_048_576
    monkeypatch.chdir(root)
    monkeypatch.setattr(Forecaster, 'measure', lambda *a: {'fits': True})
    reply = copy.deepcopy(LEAF)
    reply['checks'] = [{'kind': kind, 'node_ids': ['tests/test_large.py::test_existing'], 'goal_ids': [0, 1]}]
    check_reply(planner_request['inputs'], reply)
    assert not any('test_large.py' in item for item in reply['violations']), reply['violations']


@pytest.mark.parametrize('dimension,value', [
    ('executor', 'buildbuddy_remote_bazel'),
    ('runner_features', ['pinned_cpu_contention']),
    ('runner_features', ['native_threads_1']),
    ('artifacts', ['invocation_url']),
    ('methods', ['forced_reproduction.contention_wrapper']),
    ('protected_files', ['.github/workflows/gate.yml']),
])
def test_evidence_contract_blocks_unsupported_requirements(planner_request, monkeypatch, dimension, value):
    from tl3_workers.roles.planner import check_reply
    monkeypatch.chdir(planner_request['worktree'])
    tool = {'tool': 'run_tests', 'executor': 'prismabuild_pytest',
            'runner_features': ['base_head'], 'artifacts': ['logs', 'terminal_verdict', 'sha'],
            'methods': ['red_green'], 'protected_files': []}
    requirement = {**copy.deepcopy(tool), 'requirement': 'PB2640-2650 and PB2654 require exact remote evidence.'}
    requirement[dimension] = value
    inputs = copy.deepcopy(planner_request['inputs'])
    inputs['tool_capabilities'] = [tool]
    inputs['ingest']['evidence_contract'] = [requirement]
    proposal = copy.deepcopy(LEAF)
    proposal['evidence_contract'] = []  # Planning cannot erase an intake requirement.
    check_reply(inputs, proposal)
    assert proposal['kind'] == 'cant_meet'
    assert dimension in proposal['report']
    assert (value if isinstance(value, str) else value[0]) in proposal['report']
    assert requirement in proposal['evidence_contract']


def test_evidence_contract_requires_one_tool_to_supply_all_artifacts(planner_request, monkeypatch):
    from tl3_workers.roles.planner import check_reply
    monkeypatch.chdir(planner_request['worktree'])
    inputs = copy.deepcopy(planner_request['inputs'])
    capability = {'tool': 'run_tests', 'executor': 'prismabuild_pytest',
                  'runner_features': ['base_head'], 'artifacts': ['logs', 'terminal_verdict'],
                  'methods': ['red_green'], 'protected_files': []}
    inputs['tool_capabilities'] = [dict(capability, artifacts=['logs']),
                                   dict(capability, artifacts=['terminal_verdict'])]
    inputs['ingest']['evidence_contract'] = [dict(capability, requirement='Both artifacts from the run.')]
    reply = dict(copy.deepcopy(LEAF), evidence_contract=[])
    check_reply(inputs, reply)
    assert reply['kind'] == 'cant_meet'
    inputs['tool_capabilities'] = [capability]
    reply = dict(copy.deepcopy(LEAF), evidence_contract=[])
    check_reply(inputs, reply)
    assert reply['kind'] == 'leaf'
    assert reply['violations'] == []


def test_evidence_contract_rechecks_current_capabilities(planner_request, monkeypatch):
    from tl3_workers.roles.planner import check_reply
    monkeypatch.chdir(planner_request['worktree'])
    inputs = copy.deepcopy(planner_request['inputs'])
    capability = {'tool': 'run_tests', 'executor': 'prismabuild_pytest',
                  'runner_features': ['base_head'], 'artifacts': ['logs'],
                  'methods': ['forced_reproduction.patches'], 'protected_files': []}
    inputs['tool_capabilities'] = [capability]
    inputs['ingest']['evidence_contract'] = [dict(capability, requirement='Keep the log and force the flake.')]
    inputs['ingest']['evidence_errors'] = ['Unsupported evidence requirement: artifacts=logs.']
    reply = dict(copy.deepcopy(LEAF), evidence_contract=[])
    check_reply(inputs, reply)
    assert reply['kind'] == 'leaf'
    assert reply['evidence_errors'] == []


@pytest.mark.parametrize('separate', [False, True])
def test_source_partition_supports_test_helpers(planner_request, digests_plan, separate):
    path = Path(planner_request['worktree']) / DIGESTS
    path.write_text(path.read_text() + '\ndef _execute(result):\n    assert result.returncode == 0\n')
    planner_request['inputs']['ingest']['goals'] = [
        {'description': 'Improve helper diagnostics.', 'verification': 'test'}]
    observer = 'tests/test_observer.py' if separate else DIGESTS
    digests_plan.update(files=list(dict.fromkeys([DIGESTS, observer])), checks=[{
        'kind': 'demonstrating', 'node_ids': [f'{observer}::test_diagnostic'], 'goal_ids': [0],
        'proof': {'subjects': [f'{DIGESTS}::_execute'], 'observer_support': [],
                  'red_message': 'diagnostic missing'}}])
    validate('planner-reply', digests_plan)
    assert planned(planner_request, copy.deepcopy(digests_plan)) == []
    digests_plan['checks'][0]['proof']['subjects'] = ['outside.py::missing']
    assert any('declared files' in item for item in planned(planner_request, copy.deepcopy(digests_plan)))




def test_request_149_reports_the_reserved_bound(tmp_path, planner_request, request_149_plan):
    result = run_reply(tmp_path, planner_request, request_149_plan)
    assert result.exit_code == 0, result.stderr
    plan = next(event['reply'] for event in result.events if event['event'] == 'reply')
    violation = next(v for v in plan['violations'] if v.startswith('Invalid forcing:'))
    assert "Invalid forcing: a forcing can't set the environment variable 'MKL_NUM_THREADS'" in violation
    assert 'runner-owned' in violation
    assert plan['checks'][0]['forcing'] == request_149_plan['checks'][0]['forcing']


@pytest.mark.parametrize('cpus', [1, 4])
@pytest.mark.parametrize('stored_prompt', [False, True])
def test_request_149_compliant_plan_and_model_contract(tmp_path, planner_request, request_149_plan, monkeypatch, stored_prompt, cpus):
    planner_request['test_cpus'] = cpus
    setup = '''
from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
original_generate = FakeMessagesListChatModel._generate
def inspect_contract(self, messages, **kwargs):
    from tl3_harness.model import RESERVED_ENVIRONMENT, RESERVED_PREFIXES
    system = next(m.content for m in messages if m.type == 'system')
    inputs = json.loads(next(m.content for m in messages if m.type == 'human'))
    contract = inputs['forcing_capabilities']
    assert set(contract['reserved_environment']) == RESERVED_ENVIRONMENT
    assert tuple(contract['reserved_prefixes']) == RESERVED_PREFIXES
    cpus = inputs['tool_capabilities'][0]['runner_features'][-1].removeprefix('native_threads_')
    assert contract['runner_environment'] == {name: cpus for name in ('OMP_NUM_THREADS', 'MKL_NUM_THREADS', 'OPENBLAS_NUM_THREADS')}
    assert contract['runner_feature'] == f'native_threads_{cpus}'
    from tl3_harness.model import Forcing, HarnessInputError
    for name in contract['runner_environment']:
        try:
            Forcing.parse({'symptom': 'Reserved runner value', 'environment': {name: cpus}})
        except HarnessInputError as error:
            assert 'runner-owned' in str(error)
        else:
            raise AssertionError(f'forcing accepted {name}')
    assert all(name in system for name in RESERVED_ENVIRONMENT | set(RESERVED_PREFIXES))
    assert 'runner configuration' in system
    assert f'native_threads_{cpus}' in system
    assert all(f'{name}={cpus}' in system for name in contract['runner_environment'])
    return original_generate(self, messages, **kwargs)
FakeMessagesListChatModel._generate = inspect_contract
'''
    monkeypatch.setattr(worker_process, 'LAUNCH', setup + worker_process.LAUNCH)
    if stored_prompt:
        planner_request['prompt'] = {'version': 1, 'text': 'Plan this deflake.'}
    request_149_plan['checks'][0]['forcing']['environment'] = {}
    tool = {'tool': 'run_tests', 'executor': 'prismabuild_pytest',
            'runner_features': ['base_head', f'native_threads_{cpus}'],
            'artifacts': ['logs', 'terminal_verdict', 'cas_receipt'],
            'methods': ['forced_reproduction.patches'], 'protected_files': []}
    requirement = {**copy.deepcopy(tool), 'runner_features': [f'native_threads_{cpus}'],
                   'methods': [], 'requirement': f'Keep native thread bounds at {cpus}.'}
    planner_request['inputs']['tool_capabilities'] = [tool]
    planner_request['inputs']['ingest']['evidence_contract'] = [requirement]
    request_149_plan['evidence_contract'] = []
    result = run_reply(tmp_path, planner_request, request_149_plan)
    assert result.exit_code == 0, result.stderr
    plan = next(event['reply'] for event in result.events if event['event'] == 'reply')
    assert plan['kind'] == 'leaf'
    assert all('matching forcing rehearsal receipt' in v for v in plan['violations'])
    assert plan['violations']
    assert plan['evidence_contract'] == [requirement]
    assert plan['checks'] == request_149_plan['checks']


@pytest.mark.parametrize('name', sorted(RESERVED_ENVIRONMENT) + [prefix + 'TEST' for prefix in RESERVED_PREFIXES])
def test_request_149_keeps_reserved_protections(planner_request, request_149_plan, monkeypatch, name):
    from tl3_workers.forecast import Forecaster
    from tl3_workers.roles.planner import check_reply
    monkeypatch.chdir(planner_request['worktree'])
    monkeypatch.setattr(Forecaster, 'measure', lambda *a: {'fits': True})
    request_149_plan['checks'][0]['forcing']['environment'] = {name: '1'}
    check_reply(planner_request['inputs'], request_149_plan)
    violation = next(v for v in request_149_plan['violations'] if v.startswith('Invalid forcing:'))
    assert repr(name) in violation
    if name in {'OMP_NUM_THREADS', 'MKL_NUM_THREADS', 'OPENBLAS_NUM_THREADS'}:
        assert f'{name} is a runner-owned native-thread bound' in violation
