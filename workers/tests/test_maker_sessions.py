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
from message_invariants import ToolModel as ToolModel
from message_invariants import git as git
from message_invariants import maker_request as maker_request
from message_invariants import tool_capable_process_model as tool_capable_process_model
from message_invariants import turn as turn
from message_invariants import write_turn as write_turn


@pytest.mark.parametrize('resume_requested', [False, True])
@pytest.mark.durability
def test_adopted_fix_round_without_session_starts_fresh(tmp_path, maker_request, resume_requested):
    root = Path(maker_request['worktree'])
    (root / 'author.txt').write_text('Keep the author commit.\n')
    git(root, 'add', '.')
    git(root, 'commit', '-m', 'Author proposal')
    before = git(root, 'rev-parse', 'HEAD')
    session = tmp_path / 'missing-session.json'
    maker_request['session_file'] = str(session)
    maker_request['resume_session'] = resume_requested
    maker_request['inputs']['round'] = 1
    maker_request['inputs']['feedback'] = [{'kind': 'failed_check', 'report': 'Greeting still fails.', 'goal_ids': ['greeting']}]
    result = start_worker(tmp_path, maker_request, [write_turn(), turn(DONE)])
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]['reply']['kind'] == 'done'
    assert git(root, 'rev-parse', 'HEAD^') == before
    assert (root / 'author.txt').read_text() == 'Keep the author commit.\n'
    history = json.loads(session.read_text())
    requests = [json.loads(message['data']['content']) for message in history
                if message['type'] == 'human' and not message['data']['content'].startswith('TL3 execution budget: ')]
    assert requests == [maker_request['inputs']], 'the fresh visit has exactly its current request inputs'
    assert 'Greeting still fails.' in json.dumps(history)


@pytest.mark.parametrize('session_path_supplied', [False, True])
@pytest.mark.durability
def test_accepted_maker_session_missing_fails_closed(tmp_path, maker_request, session_path_supplied):
    root = Path(maker_request['worktree'])
    before = git(root, 'rev-parse', 'HEAD')
    session = tmp_path / 'missing-accepted-session.json'
    if session_path_supplied:
        maker_request['session_file'] = str(session)
    maker_request['resume_session'] = True
    maker_request['last_accepted_generation'] = 0
    maker_request['operation']['generation'] = 1
    maker_request['inputs']['round'] = 1
    maker_request['inputs']['feedback'] = [{'kind': 'failed_check', 'report': 'Greeting still fails.', 'goal_ids': ['greeting']}]
    result = start_worker(tmp_path, maker_request, [write_turn(), turn(DONE)])
    assert result.exit_code == 1, result.stderr
    assert result.stderr.strip() == 'tl3_workers: run failed'
    assert result.events == []
    assert git(root, 'rev-parse', 'HEAD') == before
    assert (root / 'greeting.py').read_text() == "def greet(): return 'goodbye'\n"
    assert not session.exists()
    assert not session.with_suffix('.head').exists()


@pytest.mark.durability
def test_retry_after_crash_after_commit_reuses_the_round_commit(maker_request, monkeypatch, tmp_path):
    root = Path(maker_request['worktree'])
    base = git(root, 'rev-parse', 'HEAD')
    session = tmp_path / 'session.json'
    maker_request['session_file'] = str(session)
    model = ToolModel(responses=[AIMessage(**write_turn()), AIMessage(**turn(DONE))])
    monkeypatch.setattr(runtime, 'create_model', lambda request: model)
    commit = maker._commit

    def crash_after_commit(*args):
        commit(*args)
        raise RuntimeError('crash after commit, before successful exit')

    monkeypatch.setattr(maker, '_commit', crash_after_commit)
    with pytest.raises(RuntimeError, match='crash after commit'):
        runtime.run(maker_request, lambda event: None)
    head = git(root, 'rev-parse', 'HEAD')
    assert head != base
    assert not session.exists(), 'a raised run must not save its history'
    monkeypatch.setattr(maker, '_commit', commit)
    monkeypatch.setattr(runtime, 'create_model', lambda request: ToolModel(responses=[AIMessage(**turn(DONE))]))
    events = []
    runtime.run(maker_request, events.append)
    assert events[-1]['reply'] == {**DONE, 'head': head}
    assert git(root, 'rev-parse', 'HEAD^') == base
    assert git(root, 'rev-parse', 'HEAD') == head
    assert session.exists()


@pytest.mark.parametrize('failure', ['schema', 'stream'])
@pytest.mark.durability
def test_failed_run_preserves_the_last_clean_session(maker_request, monkeypatch, tmp_path, failure):
    maker_request['inputs']['context_size'] = 150000
    session = tmp_path / 'session.json'
    maker_request['session_file'] = str(session)
    reply = {'kind': 'cant_meet', 'report': 'Need another approach.'}
    monkeypatch.setattr(runtime, 'create_model', lambda request: ToolModel(responses=[AIMessage(**turn(reply))]))
    runtime.run(maker_request, lambda event: None)
    clean = session.read_bytes()
    maker_request['resume_session'] = True
    if failure == 'schema':
        monkeypatch.setattr(runtime, 'create_model', lambda request: ToolModel(responses=[AIMessage(content='invalid json')]))
    else:
        class BrokenAgent:
            def stream(self, *args, **kwargs):
                yield {'model': {'messages': [AIMessage(content='partial history')]}}
                raise ValueError('stream failed')
        monkeypatch.setattr(runtime, 'build_agent', lambda *args, **kwargs: BrokenAgent())
    with pytest.raises(ValueError):
        runtime.run(maker_request, lambda event: None)
    assert session.read_bytes() == clean
    assert not session.with_suffix('.tmp').exists()


def test_actionable_check_feedback_matches_the_input_contract(maker_request):
    maker_request['inputs']['round'] = 1
    maker_request['inputs']['feedback'] = [{
        'kind': 'failed_check', 'report': 'The greeting assertion failed and secret.py is outside the plan.',
        'goal_ids': ['greeting'],
        'failed_tests': [{'test': 'test_greeting.py::test_hello', 'message': "assert 'goodbye' == 'hello'"}],
        'outside_paths': ['secret.py'],
        'ci_checks': [{'name': 'greeting', 'summary': 'Greeting assertion failed.', 'url': 'https://example.invalid/check/1'}],
    }]
    validate('maker-input', maker_request['inputs'])


@pytest.mark.parametrize('moved_head', [False, True])
@pytest.mark.parametrize('new_edits', [False, True])
@pytest.mark.durability
def test_rebase_needs_changes_revisit_uses_its_own_checkpoint(
        maker_request, monkeypatch, tmp_path, moved_head, new_edits):
    root = Path(maker_request['worktree'])
    maker_request['session_file'] = str(tmp_path / 'session.json')
    maker_request['inputs']['context_size'] = 150000
    monkeypatch.setattr(runtime, 'create_model', lambda request: ToolModel(
        responses=[AIMessage(**write_turn()), AIMessage(**turn(DONE))]))
    runtime.run(maker_request, lambda event: None)
    if moved_head:
        # A rewritten upstream history need not retain the first visit's starting commit.
        rebased = git(root, 'commit-tree', 'HEAD^{tree}', '-m', 'Rebased history')
        git(root, 'reset', '--hard', rebased)
    before = git(root, 'rev-parse', 'HEAD')
    maker_request['last_accepted_generation'] = maker_request['operation']['generation']
    maker_request['operation']['generation'] += 1
    maker_request['resume_session'] = True
    turns = [write_turn(content="def greet(): return 'hello again'\n")] if new_edits else []
    monkeypatch.setattr(runtime, 'create_model', lambda request: ToolModel(
        responses=[AIMessage(**message) for message in [*turns, turn(DONE)]]))
    if new_edits:
        events = []
        runtime.run(maker_request, events.append)
        assert events[-1]['reply']['kind'] == 'done'
        assert git(root, 'rev-parse', 'HEAD^') == before
    else:
        events = []
        runtime.run(maker_request, events.append)
        assert events[-1]['reply'] == {'kind': 'cant_meet', 'report': 'no code change was made'}
        assert git(root, 'rev-parse', 'HEAD') == before
    checkpoint = Path(maker_request['session_file']).with_suffix('.head')
    assert json.loads(checkpoint.read_text()) == {
        'generation': maker_request['operation']['generation'], 'head': before,
    }
    assert list(tmp_path.glob('*.head')) == [checkpoint], 'visits replace one bounded checkpoint'


@pytest.mark.parametrize('existing', [False, True])
@pytest.mark.parametrize('usage_cutoff', [False, True])
@pytest.mark.durability
def test_out_of_room_never_saves_a_conversation(maker_request, monkeypatch, tmp_path, existing, usage_cutoff):
    session = tmp_path / 'session.json'
    maker_request['session_file'] = str(session)
    clean = b'[]'
    if existing:
        session.write_bytes(clean)
    message = turn({'kind': 'out_of_room'}, input_tokens=900 if usage_cutoff else 100)
    monkeypatch.setattr(runtime, 'create_model', lambda request: ToolModel(responses=[AIMessage(**message)]))
    events = []
    runtime.run(maker_request, events.append)
    assert events[-1]['reply']['kind'] == 'out_of_room'
    assert session.read_bytes() == clean if existing else not session.exists()


def test_ci_feedback_summary_has_a_character_cap(maker_request):
    maker_request['inputs']['round'] = 1
    maker_request['inputs']['feedback'] = [{
        'kind': 'failed_check', 'report': 'CI failed.', 'goal_ids': ['greeting'],
        'ci_checks': [{'name': 'Rust', 'summary': 'x' * 1000, 'url': None}],
    }]
    validate('maker-input', maker_request['inputs'])
    maker_request['inputs']['feedback'][0]['ci_checks'][0]['summary'] += 'x'
    with pytest.raises(ValidationError):
        validate('maker-input', maker_request['inputs'])
