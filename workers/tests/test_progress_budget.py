"""#692: progress earns maker turns, with a bounded stall window and backstop."""

from copy import deepcopy
from dataclasses import asdict, replace
import json
from pathlib import Path
from types import SimpleNamespace

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
import pytest

from tl3_workers import runtime
from tl3_workers.roles import maker
from test_maker_budget import maker_request, snapshots  # noqa: F401
from message_invariants import agent_calls


class ProgressModel(FakeMessagesListChatModel):
    calls: list = []
    actions: list = []
    tokens: int = 100
    exhausted: bool = False

    def bind_tools(self, tools, **kwargs):
        self.exhausted = kwargs.get('tool_choice') == 'none'
        return self

    def _generate(self, messages, **kwargs):
        self.calls.append((deepcopy(messages), set() if self.exhausted else set(maker.ROLE.tools)))
        i = len(self.calls) - 1
        final = self.exhausted or i >= len(self.actions)
        name, args = self.actions[i] if not final else ('read', {})
        self.responses = [AIMessage(
            content=json.dumps({'kind': 'cant_meet', 'report': 'Fixture ends with work unfinished.'}) if final else '',
            tool_calls=[] if final else [{'name': name, 'args': args, 'id': str(i), 'type': 'tool_call'}],
            usage_metadata={'input_tokens': self.tokens, 'output_tokens': 1, 'total_tokens': self.tokens + 1})]
        self.i = 0
        return super()._generate(messages, **kwargs)


def run(maker_request, monkeypatch, actions, *, backstop=512, window=16, reserve=8, tokens=100):
    budget = SimpleNamespace(**{**asdict(maker.ROLE.exploration_budget), 'tool_turns': backstop,
                                'no_progress_window': window, 'finalization_turns': reserve})
    monkeypatch.setattr(maker, 'ROLE', replace(maker.ROLE, exploration_budget=budget))
    maker_request['inputs']['context_size'] = 150_000
    model = ProgressModel(responses=[], calls=[], actions=actions, tokens=tokens)
    monkeypatch.setattr(runtime, 'create_model', lambda _: model)
    events = []
    runtime.run(maker_request, events.append)
    return model, events[-1]['reply']


def test_progressing_maker_continues_past_64_edit_and_test_turns(maker_request, monkeypatch):
    from tl3_workers.tools import TOOL_FACTORIES

    def fixture_tests(tree):
        def run_tests(tests: list[str]):
            return {'status': 'completed', 'totals': {'failed': 0, 'passed': 1},
                    'tests': [{'test': tests[0], 'outcome': 'passed'}]}
        return run_tests

    monkeypatch.setitem(TOOL_FACTORIES, 'run_tests', fixture_tests)
    actions = []
    for i in range(40):
        actions.extend([('write', {'path': 'greeting.py', 'content': f'progress = {i}\n'}),
                        ('run_tests', {'tests': [f'test_greeting.py::test_{i}']})])
    model, reply = run(maker_request, monkeypatch, actions, tokens=100_000)
    assert len(agent_calls(model.calls)) == 81, 'progress and >6M input tokens must survive the former cap'
    assert 'budget_exhaustion' not in reply
    assert Path(maker_request['worktree'], 'greeting.py').read_text() == 'progress = 39\n'


def test_identical_reads_stop_after_a_full_no_progress_window(maker_request, monkeypatch):
    model, reply = run(maker_request, monkeypatch, [('read', {'path': 'greeting.py'})] * 40)
    diagnostic = reply.get('budget_exhaustion', {})
    assert diagnostic.get('dimension') == 'no_progress', reply
    assert diagnostic['tool_turns']['used'] == 17, 'first read is new, next 16 are identical'
    assert diagnostic['no_progress_window'] == 16
    assert diagnostic['no_progress_turns'] == 16
    assert diagnostic['last_progress_kind'] == 'read'
    assert snapshots(model)[-2]['phase'] == 'finalizing'


def test_trivial_progress_still_stops_at_backstop(maker_request, monkeypatch):
    # Distinct reads keep the no-progress counter at zero, including in the reserve.
    root = Path(maker_request['worktree'])
    (root / 'greeting.py').write_text(''.join(f'evidence {i}\n' for i in range(520)))
    actions = [('read', {'path': 'greeting.py', 'offset': i, 'limit': 1}) for i in range(520)]
    model, reply = run(maker_request, monkeypatch, actions)
    diagnostic = reply.get('budget_exhaustion', {})
    assert diagnostic.get('dimension') == 'backstop', reply
    assert diagnostic['tool_turns'] == {'used': 512, 'allowed': 512}
    assert diagnostic['last_progress_kind'] == 'read'
    assert len(agent_calls(model.calls)) == 513


@pytest.mark.parametrize('estimate, fits', [(90, True), (512, True), (513, False)])
def test_planner_feasibility_uses_backstop(estimate, fits):
    from tl3_workers.forecast import execution_forecast

    execution = execution_forecast(['source.py'], 1, declared=estimate, exploration=0)
    assert execution['fits'] is fits
    assert execution['allowed_tool_turns'] == 512
    assert execution['usable_tool_turns'] == 512, 'structural estimate already includes final verification'
