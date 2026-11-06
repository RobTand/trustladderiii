# ruff: noqa: F811

import json
from pathlib import Path
from jsonschema import ValidationError
from langchain_core.messages import AIMessage
import pytest
from tl3_workers import runtime
from tl3_workers.contracts import validate
from tl3_workers.roles import maker
from worker_process import start_worker
from message_invariants import DONE as DONE
from message_invariants import agent_calls
from message_invariants import ToolModel as ToolModel
from message_invariants import git as git
from message_invariants import maker_request as maker_request
from message_invariants import review_fix as review_fix
from message_invariants import tool_capable_process_model as tool_capable_process_model
from message_invariants import turn as turn
from message_invariants import write_turn as write_turn


@pytest.mark.parametrize('paths', [['outside.py'] * 101, ['é' * 4097]])
def test_outside_path_feedback_schema_is_bounded(maker_request, paths):
    maker_request['inputs']['feedback'] = [{
        'kind': 'failed_check', 'report': 'Files outside the footprint.',
        'goal_ids': ['greeting'], 'outside_paths': paths,
    }]
    # Use distinct paths so duplicate rejection cannot substitute for the item cap.
    if len(paths) > 1:
        maker_request['inputs']['feedback'][0]['outside_paths'] = [f'outside-{i}.py' for i in range(101)]
    with pytest.raises(ValidationError):
        validate('maker-input', maker_request['inputs'])


def test_review_fix_contract_accepts_named_tests_and_reasons(review_fix):
    inputs, reply = review_fix
    from tl3_workers.contracts import schema, registry
    from jsonschema import Draft202012Validator
    assert Draft202012Validator(schema('maker-input'), registry=registry()).is_valid(inputs), 'reviewed findings need identities'
    assert Draft202012Validator(schema('maker-reply'), registry=registry()).is_valid({**reply, 'head': 'c' * 40}), 'done must carry reproductions'
    maker._check_reply(inputs, reply)


def test_bounded_fix_round_contract_accepts_resolved_records_and_finding_refs(maker_request):
    from tl3_workers.contracts import schema, registry
    from jsonschema import Draft202012Validator
    inputs = maker_request['inputs']
    inputs.update(round=1, reviewed_sha='b' * 40, feedback=[
        {'kind': 'review_finding', 'id': 'review-1-0', 'reviewed_sha': 'a' * 40,
         'report': 'Greeting fails for an empty name.', 'goal_ids': ['greeting'], 'resolved_round': 1},
        {'kind': 'failed_check', 'report': 'Existing test failed.', 'goal_ids': [], 'resolved_round': 2},
        {'kind': 'review_finding', 'id': 'review-3-0', 'reviewed_sha': 'b' * 40,
         'report': 'Describe the signature in full.', 'goal_ids': ['greeting']},
    ])
    inputs['current_review_findings'] = [{'id': 'review-3-0', 'reviewed_sha': 'b' * 40}]
    assert Draft202012Validator(schema('maker-input'), registry=registry()).is_valid(inputs), 'one-line records and finding refs'
    maker._check_inputs(inputs)
    reply = {**DONE, 'reproductions': [{'finding_id': 'review-3-0', 'reason': 'A documentation fix.'}]}
    maker._check_reply(inputs, reply)
    inputs['feedback'][0]['resolved_round'] = 0
    assert not Draft202012Validator(schema('maker-input'), registry=registry()).is_valid(inputs), 'rounds start at 1'


@pytest.mark.parametrize('invalid', ['omitted', 'partial', 'duplicate', 'unknown', 'old', 'selector'])
def test_review_fix_must_cover_current_findings_exactly_once(review_fix, invalid):
    inputs, reply = review_fix
    if invalid == 'omitted':
        reply.pop('reproductions')
    elif invalid == 'partial':
        reply['reproductions'].pop()
    elif invalid == 'duplicate':
        reply['reproductions'].append(reply['reproductions'][0])
    elif invalid == 'unknown':
        reply['reproductions'][0]['finding_id'] = 'invented'
    elif invalid == 'old':
        reply['reproductions'][0]['finding_id'] = 'old-review'
    else:
        reply['reproductions'][0]['tests'] = ['test_greeting.py']
    with pytest.raises(ValueError, match='reproduc|finding|pytest'):
        maker._check_reply(inputs, reply)


def test_failed_checks_do_not_require_new_tests(maker_request):
    inputs = maker_request['inputs']
    inputs.update(round=1, feedback=[{
        'kind': 'failed_check', 'report': 'CI test failed.', 'goal_ids': ['greeting'],
    }])
    maker._check_reply(inputs, DONE)


def test_standing_rule_feedback_needs_no_goal_citation(maker_request):
    maker_request['inputs']['round'] = 1
    maker_request['inputs']['feedback'] = [{
        'kind': 'review_finding', 'report': 'Protect the test report stream.', 'goal_ids': [],
    }]
    validate('maker-input', maker_request['inputs'])
    maker._check_inputs(maker_request['inputs'])


def test_maker_uses_controller_selection_of_current_findings(maker_request):
    inputs = maker_request['inputs']
    selected = {'kind': 'review_finding', 'id': 'chosen', 'reviewed_sha': 'a' * 40,
                'report': 'Fix it', 'goal_ids': ['greeting']}
    inputs.update(round=0, feedback=[selected], current_review_findings=[selected])
    reply = {**DONE, 'reproductions': [{'finding_id': 'chosen', 'reason': 'Review this manually.'}]}
    maker._check_reply(inputs, reply)


@pytest.mark.parametrize('response', ['cant_meet', 'done', 'repair'])
def test_no_change_done_gets_one_feedback_turn(tmp_path, maker_request, monkeypatch, response):
    from langchain_core.messages import HumanMessage

    root = Path(maker_request['worktree'])
    before = git(root, 'rev-parse', 'HEAD')
    maker_request['inputs']['attempt'] = 3
    maker_request['model']['name'] = 'claude-opus-5-5'
    maker_request['inputs']['context_size'] = 150000
    maker_request['session_file'] = str(tmp_path / 'session.json')
    report = 'Cannot satisfy test_greeting.py::test_hello without changing the protected test.'
    final = {'kind': 'cant_meet', 'report': report} if response == 'cant_meet' else DONE
    turns = [turn(DONE), *([write_turn()] if response == 'repair' else []), turn(final)]
    seen = []
    class RecordingModel(ToolModel):
        def _generate(self, messages, *args, **kwargs):
            seen.append(list(messages))
            return super()._generate(messages, *args, **kwargs)
    model = RecordingModel(responses=[AIMessage(**message) for message in turns])
    monkeypatch.setattr(runtime, 'create_model', lambda _: model)
    events = []
    try:
        runtime.run(maker_request, events.append)
    except ValueError:
        pytest.fail('no-change done crashed instead of returning feedback to the maker')
    feedback = 'done requires code changes; if no code change can satisfy the failing checks, reply cant_meet with a report that names them'
    assert any(isinstance(message, HumanMessage) and message.content == feedback for message in seen[1])
    reply = events[-1]['reply']
    if response == 'repair':
        assert reply == {**DONE, 'head': git(root, 'rev-parse', 'HEAD')}
        assert git(root, 'rev-parse', 'HEAD^') == before
    else:
        assert reply == {'kind': 'cant_meet', 'report': report if response == 'cant_meet' else 'no code change was made'}
        assert git(root, 'rev-parse', 'HEAD') == before
    assert len(agent_calls(seen)) == len(turns)
    saved = json.loads(Path(maker_request['session_file']).read_text())
    assert any(message['data']['content'] == feedback for message in saved)


def test_no_change_done_recovers_through_protocol(tmp_path, maker_request):
    before = git(maker_request['worktree'], 'rev-parse', 'HEAD')
    maker_request['inputs']['attempt'] = 3
    maker_request['model']['name'] = 'claude-opus-5-5'
    reply = {'kind': 'cant_meet', 'report': 'The named regression check does not exist at base.'}
    result = start_worker(tmp_path, maker_request, [turn(DONE), turn(reply)])
    assert result.exit_code == 0, result.stderr
    assert result.events[-1] == {'event': 'reply', 'role': 'maker', 'reply': reply}
    assert git(maker_request['worktree'], 'rev-parse', 'HEAD') == before


def test_review662_reproduction_cannot_omit_assertion_reason(review_fix):
    from tl3_harness import HarnessInputError
    inputs, reply = review_fix
    reply['reproductions'][0].pop('proof', None)
    with pytest.raises(HarnessInputError, match='proof|reason'):
        maker._check_reply(inputs, reply)


def test_review662_reproduction_contract_carries_assertion_reason(review_fix):
    from tl3_workers.contracts import validate
    inputs, reply = review_fix
    reply['reproductions'][0]['proof'] = {
        'subjects': [], 'observer_support': [], 'red_message': 'empty greeting'}
    validate('maker-reply', {**reply, 'head': 'c' * 40})
    maker._check_reply(inputs, reply)


@pytest.mark.parametrize('case,pointer,kind', [
    ('goals', '/self_check', 'Edit'),
    ('maker_status', '/self_check/0', 'Edit'),
    ('downstream_status', '/self_check/0', 'Edit'),
    ('finding_coverage', '/reproductions', 'Edit'),
    ('proof', '/reproductions/0/proof', 'CantMeet'),
    ('selector', '/reproductions/0', 'CantMeet'),
    ('requested_path', '/files/0/path', 'CantMeet'),
])
def test_maker_refusals_render_and_guard_the_same_repair(review_fix, case, pointer, kind):
    from copy import deepcopy
    from tl3_workers.rules import CantMeet, Edit, apply_edit, permits

    inputs, original = review_fix
    before = deepcopy(original)
    if case == 'goals':
        before['self_check'] = []
    elif case == 'maker_status':
        before['self_check'][0] = {'goal_id': before['self_check'][0]['goal_id'],
                                   'status': 'pending', 'why': 'Deferred.'}
    elif case == 'downstream_status':
        goal_id = before['self_check'][0]['goal_id']
        next(goal for goal in inputs['goals'] if goal['id'] == goal_id)['stage'] = 'checking'
        original['self_check'][0] = {'goal_id': goal_id, 'status': 'pending', 'why': 'Owned downstream.'}
    elif case == 'finding_coverage':
        before['reproductions'] = []
    elif case == 'proof':
        before['reproductions'][0].pop('proof', None)
    elif case == 'selector':
        before['reproductions'][0]['tests'] = ['test_greeting.py']
    else:
        before = {'kind': 'needs_files', 'files': [{'path': '.', 'why': 'Needed.'}]}
    with pytest.raises(ValueError) as raised:
        maker._check_reply(inputs, before)
    violations = raised.value.rule_violations
    assert raised.value.rule_role == 'maker'
    violation = next(item for item in violations
                     if pointer in {name.value for name in item.rule.names(item.facts)})
    assert type(violation.repair).__name__ == kind
    rendered = violation.render(role='maker')
    assert json.dumps(pointer) in rendered
    assert violation.allowed.render() in rendered
    if isinstance(violation.repair, CantMeet):
        after = {'kind': 'cant_meet', 'report': 'Cannot supply the missing source facts.'}
        assert violation.allowed.value == []
        assert not permits(violations, before, original, role='maker')
    else:
        value = original['self_check'] if case == 'goals' else (
            original['self_check'][0] if case in ('maker_status', 'downstream_status') else [
                {'finding_id': item['id'], 'reason': 'Review manually without inventing assertion evidence.'}
                for item in inputs['current_review_findings']])
        assert isinstance(violation.repair, Edit)
        after = apply_edit(before, violation.repair, value)
        assert violation.allowed.accepts(value)
    assert permits(violations, before, after, role='maker')
    maker._check_reply(inputs, after)
    assert not permits(violations, before, {**after, 'kind': 'blocked'}, role='maker')
    if kind == 'Edit':
        assert not permits(violations, before, {**after, 'unrelated': True}, role='maker')


def test_maker_collects_status_and_findings_in_original_failure_order(review_fix):
    from copy import deepcopy
    from tl3_workers.rules import permits

    inputs, good = review_fix
    before = deepcopy(good)
    before['self_check'][0] = {'goal_id': before['self_check'][0]['goal_id'],
                               'status': 'pending', 'why': 'Deferred.'}
    before['reproductions'] = []
    with pytest.raises(ValueError) as raised:
        maker._check_reply(inputs, before)
    assert [item.rule.id for item in raised.value.rule_violations] == [
        'maker.maker_status', 'maker.reproductions']
    assert permits(raised.value.rule_violations, before, good, role='maker')


def test_maker_coverage_repair_owns_overlapping_status_diagnostics(review_fix):
    from copy import deepcopy
    from tl3_workers.rules import permits

    inputs, good = review_fix
    before = deepcopy(good)
    bad = {'goal_id': before['self_check'][0]['goal_id'], 'status': 'pending', 'why': 'Deferred.'}
    before['self_check'] = [bad, bad]
    with pytest.raises(ValueError) as raised:
        maker._check_reply(inputs, before)
    violations = raised.value.rule_violations
    assert [item.rule.id for item in violations] == ['maker.goals']
    assert violations[0].repair.pointer.value == '/self_check'
    assert permits(violations, before, good, role='maker')
    maker._check_reply(inputs, good)
