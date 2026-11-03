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
def test_external_alias_owner_refused_before_admission_and_survives_stop(
        rehearsal, monkeypatch, module, attribute):
    from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
    from langchain_core.messages import AIMessage
    from tl3_workers import runtime
    ledger, root, inputs, calls = rehearsal
    (root / 'pkg').mkdir()
    (root / 'pkg/__init__.py').write_text('')
    (root / 'pkg/mod.py').write_text(
        f'import {module}\nraise RuntimeError("served imports must not run in preflight")\n')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'stdlib alias')
    inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
    ledger = ForcingRehearsals(ledger.worktree, inputs)
    target = f'pkg.mod.{module}.{attribute}'
    forcing = {'symptom': 'A global alias is unsupported.', 'patches': [{'target': target, 'returns': 1000}]}
    result = ledger.run([NODE], forcing)
    assert calls == [], 'resolved external aliases must be refused before any rehearsal action'
    assert result['status'] == 'refused'
    assert result['refusal'] == {'phase': 'forcing_validation', 'target': target,
                                  'resolved_module': module, 'owner_category': 'external_module',
                                  'reason': 'outside_served_repository'}
    assert module in result['message'] and target in result['message']
    assert ledger.run([NODE], deepcopy(forcing)) == result and calls == []
    model = FakeMessagesListChatModel(responses=[
        AIMessage(content='', tool_calls=[{'name': 'rehearse_forcing', 'id': 'alias',
                                          'args': {'tests': [NODE], 'forcing': forcing}}]),
        AIMessage(content='malformed final after unsupported capability'),
    ])
    monkeypatch.setattr(runtime, 'create_model', lambda _: model)
    request = {'role': 'planner', 'worktree': str(root), 'inputs': inputs,
               'operation': {'node': 1, 'generation': 0, 'kind': 'planning'},
               'model': {'name': 'fixture', 'effort': 'high', 'budget_tokens': 32000}}
    events = []
    runtime.run(request, events.append)
    reply = events[-1]['reply']
    assert reply['kind'] == 'cant_meet' and target in reply['report'] and module in reply['report']
    assert reply['forcing_rehearsals'][0]['refusal'] == result['refusal']
    assert calls == []
