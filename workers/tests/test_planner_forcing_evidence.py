"""Planner forcing evidence contracts."""
# ruff: noqa: F811

import copy
from pathlib import Path

import pytest
from jsonschema import ValidationError

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
    reply['checks'] = [{'kind': 'demonstrating', 'goal_ids': [0, 1],
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


@pytest.mark.parametrize("separate", [False, True])
def test_helper_target_observer_plan_is_supported(planner_request, digests_plan, separate):
    path = Path(planner_request['worktree']) / DIGESTS
    path.write_text(path.read_text() + '\ndef _execute(result):\n    assert result.returncode == 0\n    return result\n')
    planner_request['inputs']['ingest']['goals'] = [
        {'description': 'Improve failed helper diagnostics while the inner pytest still fails.', 'verification': 'test'}]
    observer = 'tests/test_observer.py' if separate else DIGESTS
    digests_plan.update(files=list(dict.fromkeys([DIGESTS, observer])), checks=[{
        'kind': 'demonstrating', 'node_ids': [f'{observer}::test_new_diagnostic'], 'goal_ids': [0],
        'helper_evidence': {'target': f'{DIGESTS}::_execute', 'failure_cases': [
            {'args': [{'$attributes': {'returncode': 1, 'stdout': 'broken transfer'}}], 'kwargs': {}}]}}])
    validate('planner-reply', digests_plan)
    assert planned(planner_request, copy.deepcopy(digests_plan)) == []
    digests_plan['checks'][0]['helper_evidence']['target'] = f'{DIGESTS}::missing'
    digests_plan['files'].append('README.md')
    assert any('helper evidence' in item for item in planned(planner_request, copy.deepcopy(digests_plan)))


def test_helper_preflight_requires_installed_target_observer_capability():
    from tl3_workers.evidence import preflight
    capability = {'tool': 'run_tests', 'executor': 'prismabuild_pytest', 'runner_features': ['base_head'],
                  'artifacts': ['cas_receipt', 'terminal_verdict'], 'methods': [], 'protected_files': []}
    reply = {'kind': 'leaf', 'estimated_exploration_turns': 0, 'files': ['tests/test_helpers.py'], 'evidence_contract': [],
             'checks': [{'helper_evidence': {'target': 'tests/test_helpers.py::_execute'}}]}
    original = copy.deepcopy(reply)
    preflight({'tool_capabilities': [capability]}, reply, planning=True)
    assert reply['kind'] == 'cant_meet'
    assert 'methods=helper_target_observer' in reply['report']
    capability['methods'] = ['helper_target_observer']
    preflight({'tool_capabilities': [capability]}, original, planning=True)
    assert original['kind'] == 'leaf' and original['evidence_errors'] == []


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


@pytest.mark.parametrize('setup', ['_failed_queue', '_fixture_queue', '_factory_queue', '_World',
                                        '_missing_setup', '_needs_argument', '_unsupported_fixture'])
def test_helper_setup_is_preflighted_before_maker(planner_request, digests_plan, setup):
    path = Path(planner_request['worktree']) / DIGESTS
    path.write_text(path.read_text() + '\ndef _execute(q):\n    assert q.execute().returncode == 0\n'
                    '\ndef _failed_queue():\n    return object()\n'
                    '\ndef _needs_argument(value):\n    return value\n'
                    '\ndef _fixture_queue(tmp_path):\n    return object()\n'
                    '\ndef _factory_queue(tmp_path_factory):\n    return object()\n'
                    '\nclass _World: pass\n'
                    '\ndef _unsupported_fixture(monkeypatch):\n    return object()\n')
    planner_request['inputs']['ingest']['goals'] = [
        {'description': 'Improve helper failure text, preserving failure.', 'verification': 'test'}]
    digests_plan.update(files=[DIGESTS], checks=[{
        'kind': 'demonstrating', 'node_ids': [f'{DIGESTS}::test_new_diagnostic'], 'goal_ids': [0],
        'helper_evidence': {'target': f'{DIGESTS}::_execute', 'failure_cases': [
            {'args': [{'$setup': setup, '$call': 'execute'}], 'kwargs': {}}]}}])
    validate('planner-reply', digests_plan)
    violations = planned(planner_request, copy.deepcopy(digests_plan))
    if setup in ('_failed_queue', '_fixture_queue', '_factory_queue'):
        assert violations == []
    else:
        assert any('setup' in item for item in violations), violations


def test_helper_setup_requires_its_installed_capability():
    from tl3_workers.evidence import preflight
    capability = {'tool': 'run_tests', 'executor': 'prismabuild_pytest', 'runner_features': ['base_head'],
                  'artifacts': ['cas_receipt', 'terminal_verdict'],
                  'methods': ['helper_target_observer'], 'protected_files': []}
    reply = {'kind': 'leaf', 'estimated_exploration_turns': 0, 'files': ['tests/test_helpers.py'], 'evidence_contract': [],
             'checks': [{'helper_evidence': {'target': 'tests/test_helpers.py::_execute', 'failure_cases': [
                 {'args': [{'$setup': '_failed_queue', '$call': 'execute'}], 'kwargs': {}}]}}]}
    original = copy.deepcopy(reply)
    preflight({'tool_capabilities': [capability]}, reply, planning=True)
    assert reply['kind'] == 'cant_meet'
    assert 'methods=helper_target_observer.setup_function' in reply['report']
    capability['methods'].append('helper_target_observer.setup_function')
    preflight({'tool_capabilities': [capability]}, original, planning=True)
    assert original['kind'] == 'leaf' and original['evidence_errors'] == []


@pytest.mark.parametrize('fixture', ['tmp_path', 'tmp_path_factory', 'monkeypatch'])
def test_helper_setup_receipt_fixture_identity_is_bounded(fixture):
    from jsonschema import Draft202012Validator
    from tl3_workers.contracts import schema
    identity = {'target': 'tests/test_helpers.py::_execute',
                'base_target_sha256': 'a' * 64, 'head_target_sha256': 'b' * 64,
                'base_tree_sha256': 'c' * 64, 'head_tree_sha256': 'd' * 64,
                'observers': {'tests/test_helpers.py::test_diagnostic': 'e' * 64},
                'failure_cases': [{'args': [], 'kwargs': {}}],
                'failure_driver_sha256': 'f' * 64,
                'failure_preservation': [{'base': 'passed', 'head': 'passed'}],
                'setup_functions': {'_factory': {'source_sha256': 'e' * 64, 'fixtures': [fixture]}}}
    receipt_schema = schema('reviewer-input')['properties']['checks']['items']['properties']['helper_evidence']
    validator = Draft202012Validator(receipt_schema)
    if fixture == 'monkeypatch':
        with pytest.raises(ValidationError):
            validator.validate(identity)
    else:
        validator.validate(identity)
