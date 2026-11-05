"""Real reviewer with a scripted model, read denial, repair, and retry replay."""
import copy
from importlib.resources import files
import json
import sys
from unittest.mock import patch

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
from tl3_workers import runtime
from tl3_workers.contracts import model_facing, schema
from tl3_workers.turns import InvalidReply
from tl3_workers.tracing import failure_tags

request = json.load(sys.stdin)
head = request['inputs']['head_sha']
approve = {'verdict': 'approve', 'head_sha': head, 'blocking_findings': [], 'findings': []}
invalid = {**approve, 'head_sha': 'a' * 40}
observed = []


class Model(FakeMessagesListChatModel):
    def bind_tools(self, tools, **kwargs):
        from tl3_workers.roles import ROLES
        assert {tool.name for tool in tools} == set(ROLES['reviewer'].tools)
        return self

    def _generate(self, messages, **kwargs):
        observed.append(copy.deepcopy(messages))
        prompt = files('tl3_workers').joinpath('prompts/reviewer.md').read_text()
        assert messages[0].text == prompt + '\n\nReply with one JSON object only: no prose, no code fences. It must match this schema:\n' + json.dumps(model_facing(schema('reviewer-reply')), ensure_ascii=False)
        assert json.loads(messages[1].text) == request['inputs']
        assert request['key_file'] not in str(messages)
        if self.i == 2:
            assert messages[-1].text == ('Your reply failed validation: approval names a head other than the reviewed head\nTools are unavailable for this turn. Return one corrected JSON object matching your reply schema and nothing else.')
        return super()._generate(messages, **kwargs)


model = Model(responses=[
    AIMessage(content='', tool_calls=[{'name':'write','args':{'path':'forbidden.txt','content':'no'},'id':'denied','type':'tool_call'}]),
    AIMessage(content=json.dumps(invalid)),
    AIMessage(content=json.dumps(approve)),
])
with patch.object(runtime, 'create_model', lambda _: model):
    events = []
    runtime.run(request, events.append)
    assert events[-1] == {'event':'reply','role':'reviewer','reply':approve}
    assert len(observed) == 3
    request['operation']['generation'] += 1
    runtime.run(request, events.append)
    assert len(observed) == 3, 'retry must reuse its saved validated reply'
    assert events[-1]['reply'] == approve
    print(json.dumps({'reply':approve,'model_turns':len(observed),'replayed':True}))
    # Exhaustion must retain the shared safe diagnostic after both repair attempts.
    request.pop('session_file')
    observed.clear()
    model.responses[:] = [AIMessage(content=json.dumps(invalid))]
    model.i = 0
    try:
        runtime.run(request, lambda _: None)
    except InvalidReply as error:
        assert failure_tags(error) == {'failure_class':'InvalidReply','failure_message':'invalid reply: approval names a head other than the reviewed head; repaired reply: approval names a head other than the reviewed head'}
    else:
        raise AssertionError('invalid approval escaped repair')
