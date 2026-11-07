# ruff: noqa: F811

from copy import deepcopy
import json
import pytest
from pb_fixtures import supported_fake_runtime as supported_fake_runtime
from tl3_workers.forcing_rehearsal import ForcingRehearsals
from tl3_workers.roles import planner

from worker_process import FORCING
from worker_process import KEY
from worker_process import NODE
from worker_process import git
from worker_process import plan
from worker_process import reachable
from worker_process import rehearsal  # noqa: F401


def test_flux_worker_settings_do_not_require_pb_tools_or_environment(monkeypatch):
    from tl3_workers.pb_tests import TestSettings
    for name in ('PB_ACTION_KEY', 'PB_QUEUE_ROOT', 'PB_CONTAINER_IMAGE'):
        monkeypatch.delenv(name, raising=False)
    settings = TestSettings(image='sha256:' + '1' * 64,
                            execution={'backend': 'flux', 'profile': '/fixture/attempt/execution.json'})
    assert settings.backend == 'flux'
    assert settings.tools_path is None


def test_selected_flux_rehearsal_retains_backend_in_contract(rehearsal):
    from tl3_workers.forcing_rehearsal import ForcingRehearsals
    ledger, root, inputs, _ = rehearsal
    inputs['tests'] = {'image': 'sha256:' + '1' * 64,
                      'execution': {'backend': 'flux', 'profile': '/fixture/attempt/execution.json'}}
    restored = ForcingRehearsals(ledger.worktree, inputs, checkout=root)
    assert restored.worktree.test_settings.backend == 'flux'
    restored.active = {}
    restored.worktree.pb_identity = 'identity'
    restored.submitted('worker:' + 'a' * 64, True, 'tree')
    assert restored.active['inflight']['backend'] == 'flux'


def test_selected_flux_maker_reaches_activation_hold_without_pb_transport(rehearsal, monkeypatch):
    from tl3_workers import pb_tests
    ledger, _, _, calls = rehearsal
    ledger.worktree.test_settings = pb_tests.TestSettings(
        image='sha256:' + '1' * 64,
        execution={'backend': 'flux', 'profile': '/fixture/attempt/execution.json'})
    def forbidden(_):
        pytest.fail('selected Flux invoked a PB transport')
    monkeypatch.setattr(pb_tests, '_json_command', forbidden)
    from tl3_workers import flux_execution
    from tl3_harness.supervision import ExecutionFailure, ExecutionOutcome
    def held():
        raise ExecutionFailure(ExecutionOutcome.UNKNOWN_INCOMPLETE, 'admission', 'runtime_activation_held')
    monkeypatch.setattr(flux_execution, 'activation', held)
    monkeypatch.setattr(pb_tests, 'bounded', lambda value, **kwargs: value)
    result = pb_tests.make_test_tool(ledger.worktree)([NODE])
    assert result['status'] == 'infrastructure_error'
    assert result['execution']['termination_reason'] == 'runtime_activation_held'
    assert not calls


def test_selected_flux_maker_consumes_verified_payload_and_checkpoints_before_dispatch(rehearsal, monkeypatch, tmp_path):
    from tl3_workers import pb_tests, flux_execution
    ledger, _, _, _ = rehearsal
    worktree = ledger.worktree
    worktree.test_settings = pb_tests.TestSettings(image='sha256:' + '1' * 64,
        execution={'backend': 'flux', 'profile': '/fixture/attempt/execution.json'})
    key = 'worker:' + 'a' * 64
    retained = []
    worktree.pb_submitted = lambda key, base, tree: retained.append((key, base, tree))
    monkeypatch.setattr(flux_execution, 'activation', lambda: None)
    monkeypatch.setattr(flux_execution, 'allocate', lambda settings: ('a' * 64, tmp_path))
    monkeypatch.setattr(pb_tests, '_stage', lambda *args, **kwargs: 'b' * 40)
    monkeypatch.setattr(pb_tests, '_git', lambda *args: 'c' * 40)
    def prepare(settings, call, root, tree, contract, **kwargs):
        assert kwargs['parallel'] == 4
        assert contract['repeat'] == 3
        return {'key': key}
    monkeypatch.setattr(flux_execution, 'prepare', prepare)
    def execute(plan, **kwargs):
        assert retained == [(key, False, 'b' * 40)]
        return {'kind': 'tests', 'summary': {'status': 'completed', 'note': '',
                'tests': [{'test': NODE, 'outcome': 'passed'}],
                'totals': {'passed': 1, 'failed': 0, 'error': 0, 'skipped': 0}},
                'symptom_runs': [], 'receipts': [{'backend': 'flux', 'receipt_sha256': 'd' * 64}]}
    monkeypatch.setattr(flux_execution, 'execute', execute)
    monkeypatch.setattr(pb_tests, '_json_command', lambda command: pytest.fail('Flux invoked PB'))
    monkeypatch.setattr(pb_tests, 'bounded', lambda value, **kwargs: value)
    result = pb_tests.make_test_tool(worktree)([NODE], repeat=3, parallel=4)
    assert result['status'] == 'completed', result
    assert result['tree'] == 'b' * 40
    assert result['receipts'][0]['backend'] == 'flux'


@pytest.mark.parametrize('reached', [False, True])
def test_real_planner_tool_loop_refuses_or_binds_handoff(rehearsal, monkeypatch, reached):
    from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
    from langchain_core.messages import AIMessage
    from tl3_workers import runtime

    ledger, root, inputs, calls = rehearsal
    inputs['repair_plan_consistency'] = True
    forcing = reachable() if reached else deepcopy(FORCING)
    draft = plan(forcing)
    if not reached:
        # A failed experiment must bypass even format repair and #577 correction.
        draft['edits_tests'].append('tests/test_adoption.py::test_uncovered')
    final = json.dumps(draft) if reached else 'malformed final reply after failed work'
    model = FakeMessagesListChatModel(responses=[
        AIMessage(content='', tool_calls=[{'name': 'rehearse_forcing', 'id': 'force',
                                          'args': {'tests': [NODE], 'forcing': forcing}}]),
        AIMessage(content=final),
        AIMessage(content=json.dumps(plan(reachable()))),
    ])
    monkeypatch.setattr(runtime, 'create_model', lambda _: model)
    request = {'role': 'planner', 'worktree': str(root), 'inputs': inputs,
               'operation': {'node': 1, 'generation': 0, 'kind': 'planning'},
               'model': {'name': 'fixture', 'effort': 'high', 'budget_tokens': 32000}}
    events = []
    runtime.run(request, events.append)
    reply = events[-1]['reply']
    assert model.i == 2, 'failed work must never enter another model correction turn'
    assert len(calls) == 2
    if reached:
        assert reply['kind'] == 'leaf' and reply['violations'] == []
        assert reply['checks'] == draft['checks']
        assert reply['forcing_rehearsals'][0]['forcing'] == forcing
        assert reply['forcing_rehearsals'][0]['base_sha'] == ledger.base_sha
    else:
        assert reply['kind'] == 'cant_meet' and 'zero-hit' in reply['report']
        assert reply['violations'] and 'zero-hit' in reply['violations'][0]
        assert reply['forcing_rehearsals'][0]['forced']['tests'][0]['outcome'] == 'passed'


@pytest.mark.parametrize('stage', ['unforced', 'forced'])
@pytest.mark.parametrize('status,message', [
    ('not_available', 'Repository test settings are unavailable.'),
])
def test_rehearsal_failure_diagnostic_reaches_cant_meet(rehearsal, monkeypatch, stage, status, message):
    from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
    from langchain_core.messages import AIMessage
    from tl3_workers import forcing_rehearsal, runtime

    _, root, inputs, _ = rehearsal
    original = forcing_rehearsal.make_test_tool

    def test_tool(worktree, **kwargs):
        if bool(kwargs.get('forcing')) == (stage == 'forced'):
            return lambda *args, **options: {'status': status, 'message': message}
        return original(worktree, **kwargs)

    monkeypatch.setattr(forcing_rehearsal, 'make_test_tool', test_tool)
    model = FakeMessagesListChatModel(responses=[
        AIMessage(content='', tool_calls=[{'name': 'rehearse_forcing', 'id': 'force',
                                          'args': {'tests': [NODE], 'forcing': reachable()}}]),
        AIMessage(content='malformed reply after failed work'),
        AIMessage(content=json.dumps(plan(reachable()))),
    ])
    monkeypatch.setattr(runtime, 'create_model', lambda _: model)
    inputs['repair_plan_consistency'] = True
    request = {'role': 'planner', 'worktree': str(root), 'inputs': inputs,
               'operation': {'node': 1, 'generation': 0, 'kind': 'planning'},
               'model': {'name': 'fixture', 'effort': 'high', 'budget_tokens': 32000}}
    events = []
    runtime.run(request, events.append)
    reply = events[-1]['reply']
    assert reply['kind'] == 'cant_meet'
    assert model.i == 2, 'failed work must never enter another model correction turn'
    assert status in reply['report']
    assert message in reply['report']
    assert reply['violations']


@pytest.mark.parametrize('stage', ['unforced', 'forced'])
@pytest.mark.parametrize('boundary', ['submission', 'execution', 'retrieval', 'adapter'])
def test_rehearsal_infrastructure_exits_worker_without_reply_repair(
        rehearsal, monkeypatch, tmp_path, stage, boundary):
    from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
    from langchain_core.messages import AIMessage
    from tl3_workers import pb_tests, runtime
    _, root, inputs, calls = rehearsal
    original = pb_tests._json_command
    current_phase = None

    def command(args):
        nonlocal current_phase
        if args[1].endswith('pbrun.py'):
            current_phase = 'forced' if '--forcing' in args else 'unforced'
            if current_phase == stage and boundary == 'submission':
                raise OSError('submission fixture')
        if current_phase == stage and args[1].endswith('pbwait.py'):
            if boundary == 'retrieval':
                raise OSError('retrieval fixture')
            code, value = original(args)
            ending = value['endings'][0]
            if boundary == 'execution':
                ending.update(status='failed', succeeded=False, exit_code=137, reason='worker_lost')
            elif boundary == 'adapter':
                ending['result']['verified'] = False
            return code, value
        return original(args)

    monkeypatch.setattr(pb_tests, '_json_command', command)
    model = FakeMessagesListChatModel(responses=[
        AIMessage(content='', tool_calls=[{'name': 'rehearse_forcing', 'id': 'force',
                                          'args': {'tests': [NODE], 'forcing': reachable()}}]),
        AIMessage(content='malformed final must never be read'),
    ])
    monkeypatch.setattr(runtime, 'create_model', lambda _: model)
    facts, memory = tmp_path / 'facts.json', tmp_path / 'memory.json'
    session = tmp_path / 'planner-rehearsals.json'
    request = {'role': 'planner', 'worktree': str(root), 'inputs': inputs,
               'operation': {'node': 1, 'generation': 0, 'kind': 'planning'},
               'model': {'name': 'fixture', 'effort': 'high', 'budget_tokens': 32000},
               'trace': {'completion': str(facts)}, 'memory': {'summary_file': str(memory)},
               'session_file': str(session)}
    events = []
    with pytest.raises(Exception) as caught:
        runtime.run(request, events.append)
    error = caught.value
    assert type(error).__name__ == 'ForcingInfrastructureError'
    assert error.result['status'] == 'infrastructure_error'
    result = error.result[stage]
    assert result['execution']['phase'] == ('admission' if boundary == 'submission' else
                                           'unknown' if boundary == 'execution' else
                                           'summary' if boundary == 'adapter' else 'retrieval')
    if boundary != 'submission':
        assert result['action_key'] == KEY
    assert model.i == 1 and not any(e['event'] == 'reply' for e in events)
    assert len(calls) <= (2 if stage == 'forced' else 1)
    assert json.loads(facts.read_text())['failure']['class'] == 'ForcingInfrastructureError'
    reason = json.loads(memory.read_text())['reason']
    assert reason['failure_class'] == 'ForcingInfrastructureError'
    assert stage in reason['failure_message'] and result['execution']['phase'] in reason['failure_message']
    if boundary != 'submission':
        assert KEY in reason['failure_message']
    if boundary == 'retrieval':
        admitted = len(calls)
        monkeypatch.setattr(pb_tests, '_json_command', original)
        model.i = 0
        model.responses[1] = AIMessage(content=json.dumps(plan(reachable())))
        request['operation']['generation'] += 1
        runtime.run(request, events.append)
        assert events[-1]['reply']['kind'] == 'leaf'
        assert events[-1]['reply']['violations'] == []
        assert len(calls) == admitted + (1 if stage == 'unforced' else 0), 'recovery must reattach admitted actions'


def test_planner_explores_before_final_reply_without_format_repair(rehearsal, monkeypatch):
    from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
    from langchain_core.messages import AIMessage
    from tl3_workers import runtime
    _, root, inputs, calls = rehearsal
    good = reachable()
    model = FakeMessagesListChatModel(responses=[
        AIMessage(content='', tool_calls=[{'name': 'rehearse_forcing', 'id': 'negative',
                                          'args': {'tests': [NODE], 'forcing': deepcopy(FORCING)}}]),
        AIMessage(content='', tool_calls=[{'name': 'rehearse_forcing', 'id': 'distinct',
                                          'args': {'tests': [NODE], 'forcing': good}}]),
        AIMessage(content=json.dumps(plan(good))),
    ])
    monkeypatch.setattr(runtime, 'create_model', lambda _: model)
    request = {'role': 'planner', 'worktree': str(root), 'inputs': inputs,
               'operation': {'node': 1, 'generation': 0, 'kind': 'planning'},
               'model': {'name': 'fixture', 'effort': 'high', 'budget_tokens': 32000}}
    events = []
    runtime.run(request, events.append)
    reply = events[-1]['reply']
    assert reply['kind'] == 'leaf' and reply['violations'] == []
    assert len(calls) == 4
    from tl3_workers.forcing_rehearsal import GUIDANCE
    assert 'distinct' in GUIDANCE.lower() and 'infrastructure' in GUIDANCE.lower()
    assert 'failed rehearsal stops planning' not in planner.ROLE.tools['rehearse_forcing']


@pytest.mark.parametrize('module,attribute', [('time', 'monotonic'), ('select', 'select')])
def test_module_local_external_alias_reaches_runtime_import_validation(rehearsal, module, attribute):
    ledger, root, inputs, calls = rehearsal
    (root / 'pkg').mkdir()
    (root / 'pkg/__init__.py').write_text('')
    (root / 'pkg/mod.py').write_text(
        f'import {module}\nraise RuntimeError("served imports must not run in preflight")\n')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'local alias with import failure')
    inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
    ledger = ForcingRehearsals(ledger.worktree, inputs)
    target = f'pkg.mod.{module}.{attribute}'
    forcing = {'symptom': 'The local alias needs runtime validation.', 'patches': [{'target': target, 'returns': 1000}]}
    result = ledger.run([NODE], forcing)
    assert len(calls) == 2, 'static inspection must not execute the served module'
    assert result['status'] == 'refused'
    assert result['refusal']['reason'] == 'import_failure'
    assert result['refusal']['target'] == target
    assert ledger.run([NODE], deepcopy(forcing)) == result and len(calls) == 2
