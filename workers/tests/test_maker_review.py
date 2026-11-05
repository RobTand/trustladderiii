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
    from tl3_workers.reply_check import ReplyViolation
    inputs, reply = review_fix
    reply['reproductions'][0].pop('proof', None)
    with pytest.raises(ReplyViolation, match='proof|reason'):
        maker._check_reply(inputs, reply)


def test_review662_reproduction_contract_carries_assertion_reason(review_fix):
    from tl3_workers.contracts import validate
    inputs, reply = review_fix
    reply['reproductions'][0]['proof'] = {
        'subjects': [], 'observer_support': [], 'red_message': 'empty greeting'}
    validate('maker-reply', {**reply, 'head': 'c' * 40})
    maker._check_reply(inputs, reply)
