"""Phase restrictions preserve tool schemas through the real graph and proxy client."""

from dataclasses import replace
import json
from types import SimpleNamespace

import httpx
from langchain.agents.middleware import AgentMiddleware
from langchain_core.messages import HumanMessage, ToolMessage
import pytest

from tl3_workers import runtime
from tl3_workers.exploration import ExplorationBudget, ExplorationMiddleware, FINALIZATION_TOOLS
from tl3_workers.roles import ROLES
from tl3_workers.tools import Worktree
from tl3_workers.turns import TurnControl
from worker_process import response_frames


TOOL_ROLES = [name for name, role in ROLES.items() if role.tools]
BUDGET_ROLES = [name for name, role in ROLES.items() if role.exploration_budget]


def wire_agent(tmp_path, role, respond):
    client = httpx.Client(transport=httpx.MockTransport(respond))
    model = runtime.ProxyChatOpenAI(
        base_url="http://model.invalid/v1", model="fixture-model", api_key="fixture-only-key",
        streaming=True, stream_usage=True, use_responses_api=False, max_retries=0, http_client=client,
    )
    control = TurnControl()
    agent = runtime.build_agent(model, role, Worktree(tmp_path), control=control)
    return client, agent, control


@pytest.mark.parametrize("role_name", list(ROLES))
def test_wire_tools_identical_on_tool_turn_final_and_repair(tmp_path, role_name):
    sent = []
    (tmp_path / "evidence.txt").write_text("retained evidence\n")

    def respond(request):
        sent.append(json.loads(request.content))
        if len(sent) == 1 and role_name in TOOL_ROLES:
            return response_frames("", tool_calls=[{
                "index": 0, "id": "read-1", "type": "function",
                "function": {"name": "read", "arguments": json.dumps({"path": "evidence.txt"})}}])
        return response_frames('{"fixture":true}')

    client, agent, control = wire_agent(tmp_path, ROLES[role_name], respond)
    history = [HumanMessage(content="Read the evidence, then return JSON.")]
    with client:
        runtime._run_agent(agent, history, lambda _: None)
        if control.allowance:
            control.allowance.turns = control.allowance.budget.tool_turns
            runtime._run_agent(agent, history, lambda _: None)
        with control.repair(history, ValueError("fixture validation failure")):
            runtime._run_agent(agent, history, lambda _: None)
    definitions = [json.dumps(body.get("tools"), ensure_ascii=False).encode() for body in sent]
    assert all(value == definitions[0] for value in definitions), "phase changes must preserve tools and order"
    if role_name in TOOL_ROLES:
        assert sent[0]["tools"]
        assert sent[-1].get("tool_choice") == "none", "repair disables calling without removing definitions"
        if control.allowance:
            assert sent[-2].get("tool_choice") == "none", "FINISH disables calling without removing definitions"


@pytest.mark.parametrize("role_name", BUDGET_ROLES)
def test_finish_choice_is_none(tmp_path, role_name):
    sent = []

    def respond(request):
        sent.append(json.loads(request.content))
        return response_frames('{"fixture":true}')

    client, agent, control = wire_agent(tmp_path, ROLES[role_name], respond)
    control.allowance.turns = control.allowance.budget.tool_turns
    with client:
        runtime._run_agent(agent, [HumanMessage(content="Finish now.")], lambda _: None)
    assert sent[0].get("tool_choice") == "none"
    assert sent[0].get("tools"), "the no-calls choice must retain definitions"


@pytest.mark.parametrize("role_name", TOOL_ROLES)
def test_repair_dispatch_refuses_even_if_provider_returns_a_call(role_name):
    control = TurnControl()
    executed = []
    request = SimpleNamespace(tool_call={"name": "read", "args": {}, "id": "forbidden"})
    assert type(control).wrap_tool_call is not AgentMiddleware.wrap_tool_call, "repair requires a dispatch guard"
    with control.repair([], ValueError("invalid reply")):
        result = control.wrap_tool_call(request, lambda request: executed.append(request))
    assert not executed, "a repair phase must refuse at dispatch"
    assert isinstance(result, ToolMessage) and result.status == "error"
    assert result.tool_call_id == "forbidden"


@pytest.mark.parametrize("role_name", BUDGET_ROLES)
def test_finish_dispatch_refuses_even_if_provider_returns_a_call(role_name):
    allowance = ExplorationMiddleware(ExplorationBudget(), role_name)
    allowance.final = True
    executed = []
    request = SimpleNamespace(tool_call={"name": "read", "args": {}, "id": "forbidden"})
    result = allowance.wrap_tool_call(request, lambda request: executed.append(request))
    assert not executed, "FINISH must refuse tools for every role"
    assert isinstance(result, ToolMessage) and result.status == "error"
    assert result.tool_call_id == "forbidden"


@pytest.mark.parametrize("name", sorted(set(ROLES["maker"].tools) - FINALIZATION_TOOLS))
def test_maker_finalization_keeps_wire_definitions_and_refuses_mutations(tmp_path, name):
    sent = []
    (tmp_path / "evidence.txt").write_text("preserved\n")

    def respond(request):
        sent.append(json.loads(request.content))
        if len(sent) == 2:
            return response_frames("", tool_calls=[{
                "index": 0, "id": "forbidden", "type": "function",
                "function": {"name": name, "arguments": json.dumps({"path": "evidence.txt", "content": "forbidden"})}}])
        return response_frames('{"fixture":true}')

    role = replace(ROLES["maker"], exploration_budget=ExplorationBudget(tool_turns=8, input_tokens=6_000_000,
                                                                       finalization_turns=4))
    client, agent, control = wire_agent(tmp_path, role, respond)
    history = [HumanMessage(content="Return JSON.")]
    with client:
        runtime._run_agent(agent, history, lambda _: None)
        control.allowance.turns = 4
        runtime._run_agent(agent, history, lambda _: None)
    assert all(body.get("tools") == sent[0].get("tools") for body in sent), "finalization retains all definitions"
    assert all(body.get("tool_choice") in (None, "auto") for body in sent), "no allowed_tools proxy filtering"
    refused = [m for m in history if isinstance(m, ToolMessage) and m.tool_call_id == "forbidden"]
    assert len(refused) == 1 and refused[0].status == "error"
    assert "finalization reserve" in refused[0].text
    assert (tmp_path / "evidence.txt").read_text() == "preserved\n"
