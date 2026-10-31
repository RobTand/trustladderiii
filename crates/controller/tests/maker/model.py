"""Real maker graph with a scripted model, recorded checks and fake recall."""
import json
from pathlib import Path
import runpy
import socket
import sys
from unittest.mock import patch

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
from tl3_workers import runtime, recall, tools
from tl3_workers.roles import maker

mode = sys.argv[1]


class Model(FakeMessagesListChatModel):
    def bind_tools(self, tools, **kwargs):
        from tl3_workers.roles import ROLES
        assert {tool.name: tool.description for tool in tools} in [dict(ROLES[role].tools) for role in ('maker', 'reviewer')]
        return self


def call(name, args):
    return AIMessage(content='', tool_calls=[{'name': name, 'args': args, 'id': name, 'type': 'tool_call'}])


def create_model(request):
    inputs = request['inputs']
    if request['role'] == 'reviewer':
        record = Path(request['worktree']).parents[1] / 'reviewer-requests' / f"request-{request['operation']['generation']}.json"
        record.parent.mkdir(parents=True, exist_ok=True)
        record.write_text(json.dumps(request))
        reply = ({'verdict': 'approve', 'head_sha': inputs['head_sha'], 'blocking_findings': [], 'findings': []}
                 if 'review_fixes' in inputs else {
                     'verdict': 'changes', 'blocking_findings': [
                         {'citation': {'kind': 'goal', 'id': 'greeting'}, 'problem': 'Empty names fail.', 'evidence': 'greet has no empty-name coverage.'},
                         {'citation': {'kind': 'goal', 'id': 'greeting'}, 'problem': 'Explain the greeting.', 'evidence': 'The documentation omits the greeting.'},
                     ], 'findings': []})
        if mode != 'review-feedback':
            return Model(responses=[AIMessage(content=json.dumps(reply))])
        blockers = [
            {'citation': {'kind': 'goal', 'id': 'greeting'},
             'problem': 'The greeting lacks context.', 'evidence': 'greeting.py returns only hello.'},
            {'citation': {'kind': 'standing_rule', 'id': 'test-report-integrity'},
             'problem': 'The report stream is overwritten.', 'evidence': 'The reporter replaces stdout.'},
        ]
        blockers.extend({'citation': {'kind': 'goal', 'id': 'greeting'},
                         'problem': f'Problem {i} ' + 'é' * 2000,
                         'evidence': f'Evidence {i} ' + '界' * 2000} for i in range(18))
        return Model(responses=[AIMessage(content=json.dumps({
            'verdict': 'changes', 'blocking_findings': blockers,
            'findings': [{'problem': 'Optional style note', 'evidence': 'Not a blocker.'}],
        }))])
    record = Path(request['session_file']).parents[3] / "maker-requests" / f"request-{request['operation']['generation']}.json"
    record.parent.mkdir(parents=True, exist_ok=True)
    record.write_text(json.dumps(request))
    if mode in ('killed-after-commit', 'session-save-fails', 'twice-killed', 'fix-killed'):
        marker = record.parent / 'failure-injected'
        failures = int(marker.read_text()) if marker.exists() else 0
        inject = failures < (2 if mode == 'twice-killed' else 1)
        if inject and (mode != 'fix-killed' or inputs['round'] == 1):
            if mode != 'session-save-fails':
                commit = maker._commit
                def kill_after_commit(*args):
                    commit(*args)
                    marker.write_text(str(failures + 1))
                    import os, signal
                    os.kill(os.getpid(), signal.SIGKILL)
                maker._commit = kill_after_commit
            else:
                save = maker._save
                def fail_session_save(path, value):
                    if path.suffix == '.json':
                        marker.write_text(str(failures + 1))
                        raise OSError('injected session save failure')
                    save(path, value)
                maker._save = fail_session_save
    if mode == 'bad-schema':
        return Model(responses=[AIMessage(content='{"kind":"done"}')])
    if mode == 'blocked':
        # The test releases or kills this process through its supervisor record.
        import threading
        threading.Event().wait()
    if mode == 'cant-meet':
        return Model(responses=[AIMessage(content=json.dumps({'kind': 'cant_meet', 'report': 'The goal cannot be implemented.'}))])
    if mode == 'out-of-room':
        return Model(responses=[AIMessage(content='stop', usage_metadata={'input_tokens': inputs['context_size'], 'output_tokens': 1, 'total_tokens': inputs['context_size'] + 1})])
    if mode == 'needs-files' and 'helper.py' not in inputs['plan']['files']:
        return Model(responses=[AIMessage(content=json.dumps({'kind': 'needs_files', 'files': [{'path': 'helper.py', 'why': 'The shared greeting is here.'}]}))])
    path = 'outside.py' if mode == 'outside' else 'greeting.py'
    content = f"def greet(): return 'hello'\n# {mode} attempt {inputs['attempt']} round {inputs['round']}\n"
    if mode == 'rebase-edits':
        content += f"# visit {request['operation']['generation']}\n"
    reply = {'kind': 'done', 'self_check': [{'goal_id': goal['id'], 'status': 'met', 'evidence': 'Recorded self-check passed; check greeting.'} for goal in inputs['goals']]}
    reproducing_turns = []
    if mode == 'review-fix' and inputs['round'] == 1:
        findings = [finding for finding in inputs['feedback'] if finding.get('reviewed_sha') == inputs.get('reviewed_sha')]
        reply['reproductions'] = [
            {'finding_id': findings[0]['id'], 'tests': ['test_greeting.py::test_empty']},
            {'finding_id': findings[1]['id'], 'reason': 'This is a documentation correction.'},
        ]
        reproducing_turns = [call('write', {'path': 'test_greeting.py', 'content': "def test_hello():\n    from greeting import greet\n    assert greet() == 'hello'\ndef test_empty():\n    from greeting import greet\n    assert greet() == 'hello'\n"})]
    if mode == 'review-feedback' and inputs['round'] == 1:
        reply['reproductions'] = [{'finding_id': f['id'], 'reason': 'This is a report integrity correction.'}
                                  for f in inputs['feedback'] if f.get('reviewed_sha') == inputs.get('reviewed_sha')]
    if mode == 'unmet':
        reply['self_check'][0] = {'goal_id': inputs['goals'][0]['id'], 'status': 'not_met', 'why': 'The interface is unresolved.'}
    return Model(responses=[
        call('recall', {'query': 'greeting decisions'}),
        *reproducing_turns,
        call('write', {'path': path, 'content': content}),
        call('run_tests', {'tests': ['test_greeting.py::test_hello']}),
        AIMessage(content=json.dumps(reply)),
    ])


def fake_recall(endpoint, bank, query):
    assert bank == 'tl3'
    assert endpoint == 'http://127.0.0.1:8890/' and query == 'greeting decisions'
    return {'results': [{'id': 'fixture-memory', 'text': 'Keep the greet interface.'}]}


def recorded_tests(worktree):
    def run_tests(tests: list[str] | None = None) -> dict:
        assert tests == ['test_greeting.py::test_hello']
        if mode == 'external-wait':
            import threading
            worktree.progress_emit({'event': 'progress', 'unit': 'external_wait',
                                    'wait': {'upstream': 'prismabuild', 'key': 'a' * 64}})
            threading.Event().wait()
        return {'status': 'completed', 'passed': 1, 'failed': 0, 'skipped': 0,
                'action_key': 'a' * 64, 'custody': 'recorded-fixture'}
    return run_tests


def no_network(*args, **kwargs):
    raise AssertionError('network forbidden in maker acceptance')


run_worker = runtime.run


def run_with_mismatch(request, emit):
    def emit_reply(event):
        marker = Path(request.get('session_file', request['worktree'])).parent / 'mismatch-injected'
        if mode == 'self-check-mismatch' and event['event'] == 'reply' and not marker.exists():
            # Corrupt a schema-valid reply after the real worker has committed.
            event['reply']['self_check'][0]['goal_id'] = 'unexpected-goal'
            marker.touch()
        if mode == 'self-check-mismatch' and event['event'] == 'reply':
            marker = Path(request['session_file']).parent / 'mismatch-injected'
            if not marker.exists():
                # Corrupt a schema-valid reply after the real worker has committed.
                event['reply']['self_check'][0]['goal_id'] = 'unexpected-goal'
                marker.touch()
        emit(event)
    return run_worker(request, emit_reply)


with patch.object(runtime, 'create_model', create_model), \
     patch.object(runtime, 'run', run_with_mismatch), \
     patch.object(recall, 'recall', fake_recall), \
     patch.dict(tools.TOOL_FACTORIES, {'run_tests': recorded_tests}), \
     patch.object(socket.socket, 'connect', no_network), \
     patch.object(socket.socket, 'connect_ex', no_network), \
     patch.object(socket, 'getaddrinfo', no_network):
    sys.argv = ['tl3_workers', 'run']
    runpy.run_module('tl3_workers', run_name='__main__')
