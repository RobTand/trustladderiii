"""Real postmortem orchestration and Rust checker, with scripted judges only."""
import json
from pathlib import Path
import socket
import sys
from unittest.mock import patch

from langchain_core.messages import AIMessage
from mlflow.entities import Feedback
from tl3_workers import judges
from tl3_workers.roles import postmortem

payload = json.load(sys.stdin)
reply = payload['reply']
workdir = Path(payload['worktree'])
request = {'worktree': str(workdir), 'model': {'name': reply['models']['postmortem'],
           'base_url': 'http://model.invalid/v1'}, 'inputs': {
    'deadline_secs': 60, 'trace_tags': {}, 'case': {}, 'record': {'id': 9},
    'grade_model': reply['models']['grades'], 'rubric': {'version': 1}, 'release': 'b' * 40}}


class Source:
    def __init__(self, settings):
        pass

    def find(self, tags, limit):
        return []


class Model:
    def __init__(self):
        self.repairs = 0

    def invoke(self, history):
        self.repairs += 1
        assert 'Your reply failed validation:' in history[-1].text
        assert 'Tools are unavailable for this turn.' in history[-1].text
        assert not (workdir / 'judged.json').exists()
        return AIMessage(content=json.dumps(reply['postmortem']))


def offline(*args, **kwargs):
    raise AssertionError('network forbidden in postmortem contract')


malformed = dict(reply['postmortem'], went_wrong=['The maker skipped a test.'])
model = Model()
with patch.object(postmortem, 'SOURCE', Source), \
     patch.object(judges, 'configure', lambda *args: None), \
     patch.object(judges, 'grade_judges', lambda *args: {}), \
     patch.object(judges, 'postmortem_judge', lambda *args: lambda **kwargs: Feedback(value=malformed)), \
     patch.object(socket.socket, 'connect', offline), \
     patch.object(socket, 'getaddrinfo', offline):
    result = postmortem.run(model, request, None, lambda event: None)
    assert result == reply
    assert postmortem.run(model, request, None, lambda event: None) == reply
    assert model.repairs == 1
    assert json.loads((workdir / 'reply.json').read_text()) == reply
print(json.dumps(result))
