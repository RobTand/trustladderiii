# ruff: noqa: F811

import json
from pathlib import Path
from langchain_core.messages import AIMessage
import pytest
from message_invariants import assert_tool_pairs
from tl3_workers import runtime
from tl3_workers.roles import ROLES
from message_invariants import DONE as DONE
from message_invariants import ToolModel as ToolModel
from message_invariants import git as git
from message_invariants import maker_request as maker_request
from message_invariants import tool_capable_process_model as tool_capable_process_model
from message_invariants import turn as turn
from message_invariants import write_turn as write_turn


@pytest.mark.parametrize('resume', [False, True])
def test_long_maker_session_bounds_context_and_commits(tmp_path, maker_request, monkeypatch, resume):
    from langchain_core.messages import HumanMessage, ToolMessage, messages_to_dict

    root = Path(maker_request['worktree'])
    before = git(root, 'rev-parse', 'HEAD')
    # Match the live maker's 50 tool turns, using real confined reads and writes.
    (root / 'evidence.txt').write_text(('source evidence ' * 30 + '\n') * 60)
    git(root, 'add', 'evidence.txt')
    git(root, 'commit', '--quiet', '-m', 'Repository evidence fixture')
    before = git(root, 'rev-parse', 'HEAD')
    maker_request['inputs']['context_size'] = 10_000_000
    maker_request['session_file'] = str(tmp_path / 'session.json')
    if resume:
        maker_request['resume_session'] = True
        Path(maker_request['session_file']).write_text(json.dumps(messages_to_dict([
            HumanMessage(content='Previous visit: preserve the public interface.'),
            AIMessage(content='Previous visit completed.'),
        ])))
    observed = []
    script = [turn(calls=[{'name': 'read', 'args': {'path': 'evidence.txt'},
                           'id': str(i), 'type': 'tool_call'}]) for i in range(48)]
    script += [write_turn(), turn(calls=[{'name': 'read', 'args': {'path': 'greeting.py'},
                                         'id': 'verify', 'type': 'tool_call'}]), turn(DONE)]

    class RecordingMaker(ToolModel):
        def _generate(self, messages, **kwargs):
            observed.append(list(messages))
            return super()._generate(messages, **kwargs)

    monkeypatch.setattr(runtime, 'create_model', lambda _: RecordingMaker(
        responses=[AIMessage(**message) for message in script]))
    events = []
    runtime.run(maker_request, events.append)
    for messages in observed:
        assert_tool_pairs(messages)
    assert len(observed) == 51, 'a long maker session can finish its planned edits'
    assert max(sum(len(json.dumps(m.model_dump(), ensure_ascii=False).encode())
                   for m in messages if isinstance(m, (AIMessage, ToolMessage)))
               for messages in observed) > 65_536, 'history grows beyond the former rolling window'
    for previous, following in zip(observed, observed[1:]):
        assert [m.model_dump() for m in following[:len(previous)]] == [m.model_dump() for m in previous], \
            'history below the context threshold preserves its exact prefix'
    request_text = json.dumps(maker_request['inputs'], ensure_ascii=False)
    assert all(any(m.content == request_text for m in messages if isinstance(m, HumanMessage))
               for messages in observed), 'the current request survives across resumed visits'
    assert next(message.text for message in reversed(observed[-1])
                if isinstance(message, ToolMessage)) == "def greet(): return 'hello'\n", 'the newest read stays whole'
    reply = events[-1]['reply']
    assert reply == {**DONE, 'head': git(root, 'rev-parse', 'HEAD')}
    assert reply['head'] != before
    assert git(root, 'show', 'HEAD:greeting.py') == "def greet(): return 'hello'"
    assert git(root, 'status', '--porcelain') == ''


@pytest.mark.parametrize('final_done', [False, True])
def test_maker_budget_exhaustion_finishes_without_claiming_unmade_changes(maker_request, monkeypatch,
                                                                         final_done):
    observed = []
    maker_request['inputs']['context_size'] = 1_000_000
    final = DONE if final_done else {'kind': 'cant_meet', 'report': 'The work is incomplete.'}

    class EndlessMaker(ToolModel):
        def bind_tools(self, tools, **kwargs):
            offered = {tool.name: tool.description for tool in tools}
            expected = ROLES["maker"].tools
            verification = {name: expected[name] for name in {"list", "read", "glob", "grep", "diff", "run_tests"}}
            assert offered in (expected, verification)
            return self

        def _generate(self, messages, **kwargs):
            observed.append(list(messages))
            exhausted = any('budget is exhausted' in m.text for m in messages if m.type == 'human')
            response = turn(final) if exhausted or len(observed) >= 75 else turn(calls=[{
                'name': 'read', 'args': {'path': 'greeting.py'}, 'id': str(len(observed)),
                'type': 'tool_call'}])
            self.responses = [AIMessage(**response)]
            self.i = 0
            return super()._generate(messages, **kwargs)

    monkeypatch.setattr(runtime, 'create_model', lambda _: EndlessMaker(responses=[]))
    events = []
    runtime.run(maker_request, events.append)
    assert len(observed) == 65, 'maker allows 64 tool turns and one final answer'
    reply = events[-1]['reply']
    assert reply['kind'] == 'cant_meet'
    assert reply['budget_exhaustion']['dimension'] == 'tool_turns'
    assert reply['budget_exhaustion']['tool_turns'] == {'used': 64, 'allowed': 64}
    assert reply['budget_exhaustion']['unfinished_obligations']
    assert 'execution_budget_exhausted' in reply['report']
