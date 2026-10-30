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

mode = sys.argv[1]


class Model(FakeMessagesListChatModel):
    def bind_tools(self, tools, **kwargs):
        from tl3_workers.roles import ROLES
        assert {tool.name: tool.description for tool in tools} == dict(ROLES['maker'].tools)
        return self


def call(name, args):
    return AIMessage(content='', tool_calls=[{'name': name, 'args': args, 'id': name, 'type': 'tool_call'}])


def create_model(request):
    inputs = request['inputs']
    record = Path(request['session_file']).parent / f"request-{request['operation']['generation']}.json"
    record.parent.mkdir(parents=True, exist_ok=True)
    record.write_text(json.dumps(request))
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
    content = f"def greet(): return 'hello'\n# attempt {inputs['attempt']} round {inputs['round']}\n"
    reply = {'kind': 'done', 'self_check': [{'goal_id': goal['id'], 'status': 'met', 'evidence': 'Recorded self-check passed; check greeting.'} for goal in inputs['goals']]}
    if mode == 'unmet':
        reply['self_check'][0] = {'goal_id': inputs['goals'][0]['id'], 'status': 'not_met', 'why': 'The interface is unresolved.'}
    return Model(responses=[
        call('recall', {'query': 'greeting decisions'}),
        call('write', {'path': path, 'content': content}),
        call('run_tests', {'tests': ['test_greeting.py::test_hello']}),
        AIMessage(content=json.dumps(reply)),
    ])


def fake_recall(endpoint, query):
    assert endpoint == 'http://127.0.0.1:8890/' and query == 'greeting decisions'
    return {'results': [{'id': 'fixture-memory', 'text': 'Keep the greet interface.'}]}


def recorded_tests(worktree):
    def run_tests(tests: list[str] | None = None) -> dict:
        assert tests == ['test_greeting.py::test_hello']
        return {'status': 'completed', 'passed': 1, 'failed': 0, 'skipped': 0,
                'action_key': 'a' * 64, 'custody': 'recorded-fixture'}
    return run_tests


def no_network(*args, **kwargs):
    raise AssertionError('network forbidden in maker acceptance')


with patch.object(runtime, 'create_model', create_model), \
     patch.object(recall, 'recall', fake_recall), \
     patch.dict(tools.TOOL_FACTORIES, {'run_tests': recorded_tests}), \
     patch.object(socket.socket, 'connect', no_network), \
     patch.object(socket.socket, 'connect_ex', no_network), \
     patch.object(socket, 'getaddrinfo', no_network):
    sys.argv = ['tl3_workers', 'run']
    runpy.run_module('tl3_workers', run_name='__main__')
