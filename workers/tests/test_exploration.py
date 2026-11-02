"""Budget checks use the real graph and confined tools, without a provider."""
import json

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage, ToolMessage
import pytest

from tl3_workers import runtime
from tl3_workers.contracts import validate
from test_workers import AGREE, BRIEF
from message_invariants import assert_tool_pairs


@pytest.mark.parametrize("role,reply", [("ingest", BRIEF), ("audit", AGREE)])
@pytest.mark.parametrize("large_input", [False, True])
def test_exploration_finishes_within_turn_and_input_budgets(tmp_path, monkeypatch, role, reply,
                                                          large_input):
    seen = []
    offered = []
    (tmp_path / "evidence.txt").write_text("evidence\n" * 2000)

    class Explorer(FakeMessagesListChatModel):
        def bind_tools(self, tools, **kwargs):
            offered.append(bool(tools))
            return self

        def _generate(self, messages, **kwargs):
            seen.append(messages)
            # The guard prevents an old runtime looping indefinitely in the red run.
            final = (len(seen) == 9 or (offered and not offered[-1])
                     or any("finish" in m.text.lower() and "budget" in m.text.lower()
                            for m in messages if m.type == "human"))
            content = json.dumps(reply) if final else ""
            calls = [] if final else [{"name": "read", "args": {"path": "evidence.txt"},
                                       "id": str(len(seen)), "type": "tool_call"}]
            self.responses = [AIMessage(content=content, tool_calls=calls, usage_metadata={
                "input_tokens": 120_000 if large_input else 1000,
                "output_tokens": 50, "total_tokens": 120_050 if large_input else 1050})]
            self.i = 0
            return super()._generate(messages, **kwargs)

    model = Explorer(responses=[AIMessage(content="")])
    monkeypatch.setattr(runtime, "create_model", lambda _: model)
    inputs = {"issue": {"title": "Explore", "body": "x" * 120_000 if large_input else "Fix it"},
              "repo": {"readme": "fixture", "file_list": ["evidence.txt"]}}
    if role == "audit":
        inputs["ingest"] = BRIEF
    events = []
    runtime.run({"role": role, "worktree": str(tmp_path), "inputs": inputs}, events.append)
    for messages in seen:
        assert_tool_pairs(messages)
    assert len(seen) <= 5, "at most four exploration turns and one final answer"
    assert len(seen) * (120_000 if large_input else 1000) <= 300_000
    assert events[-1]["reply"] == reply
    validate(role + "-reply", events[-1]["reply"])
    # Before compaction, complete evidence accumulates inside the context allowance.
    assert max(sum(len(m.text.encode()) for m in messages if isinstance(m, ToolMessage))
               for messages in seen) <= 150_000 * 3 * 0.7


def test_failed_run_summary_keeps_safe_diagnostic(tmp_path, monkeypatch):
    path = tmp_path / "memory.json"
    monkeypatch.setattr(runtime, "create_model", lambda _: FakeMessagesListChatModel(
        responses=[AIMessage(content=json.dumps({**BRIEF, "memory_ids": ["not-supplied"]}))]))
    with pytest.raises(ValueError, match="reply cites a memory it was not given"):
        runtime.run({"role": "ingest", "worktree": str(tmp_path), "inputs": {},
                     "memory": {"summary_file": str(path)}}, lambda _: None)
    assert json.loads(path.read_text())["reason"] == {
        "failure_class": "ValueError", "failure_message": "reply cites a memory it was not given"}


@pytest.mark.parametrize("role,reply", [("ingest", BRIEF), ("audit", AGREE)])
def test_parallel_results_are_bounded_and_marked_as_clipped(tmp_path, monkeypatch, role, reply):
    observed = []
    (tmp_path / "large.txt").write_text(("evidence " * 100 + "\n") * 200)

    class InspectModel(FakeMessagesListChatModel):
        def _generate(self, messages, **kwargs):
            observed.append(messages)
            return super()._generate(messages, **kwargs)

    calls = [{"name": "read", "args": {"path": "large.txt"}, "id": str(i), "type": "tool_call"}
             for i in range(5)]
    monkeypatch.setattr(runtime, "create_model", lambda _: InspectModel(responses=[
        AIMessage(content="", tool_calls=calls), AIMessage(content=json.dumps(reply))]))
    events = []
    runtime.run({"role": role, "worktree": str(tmp_path), "inputs": {"context_size": 20_000}}, events.append)
    for messages in observed:
        assert_tool_pairs(messages)
    results = [m for m in observed[-1] if isinstance(m, ToolMessage)]
    assert {m.tool_call_id for m in results} == {str(i) for i in range(5)}
    assert sum(len(m.text.encode()) for m in results) <= 32_768
    assert any("clipped" in m.text for m in observed[-1] if m.type == "human")
    assert events[-1]["reply"] == reply


@pytest.mark.parametrize("role,reply", [("ingest", BRIEF), ("audit", AGREE)])
@pytest.mark.parametrize("reported,orientation_bytes", [(True, 88_000), (False, 88_000), (True, 200_000)])
def test_real_orientation_allows_four_exploration_turns(tmp_path, monkeypatch, role, reply, reported,
                                                       orientation_bytes):
    observed = []
    (tmp_path / "evidence.txt").write_text("evidence")

    class OrientationModel(FakeMessagesListChatModel):
        def _generate(self, messages, **kwargs):
            observed.append(messages)
            exhausted = any("budget is exhausted" in m.text for m in messages if m.type == "human")
            final = exhausted or len(observed) == 5
            self.responses = [AIMessage(
                content=json.dumps(reply) if final else "",
                tool_calls=[] if final else [{"name": "read", "args": {"path": "evidence.txt"},
                                              "id": str(len(observed)), "type": "tool_call"}],
                usage_metadata={"input_tokens": 25_000, "output_tokens": 10,
                                "total_tokens": 25_010} if reported else None)]
            self.i = 0
            return super()._generate(messages, **kwargs)

    monkeypatch.setattr(runtime, "create_model", lambda _: OrientationModel(responses=[]))
    events = []
    runtime.run({"role": role, "worktree": str(tmp_path), "inputs": {"orientation": "x" * orientation_bytes}},
                events.append)
    assert len(observed) == 5, "orientation must leave room for all four exploration rounds"
    for messages in observed:
        assert_tool_pairs(messages)
    assert events[-1]["reply"] == reply


def test_newest_large_write_keeps_its_call_and_result():
    from langchain_core.messages import HumanMessage
    from tl3_workers.exploration import retained

    messages = [HumanMessage(content="Implement the change"),
                AIMessage(content="", tool_calls=[{"name": "read", "args": {"path": "old.py"},
                                                   "id": "old", "type": "tool_call"}]),
                ToolMessage(content="old evidence", tool_call_id="old"),
                AIMessage(content="", tool_calls=[{"name": "write", "args": {
                    "path": "new.py", "content": "new file content\n" * 20_000},
                    "id": "latest", "type": "tool_call"}]),
                ToolMessage(content="wrote new.py", tool_call_id="latest")]
    result = retained(messages, 4096)
    assert_tool_pairs(result)
    latest = [m for m in result if isinstance(m, ToolMessage) and m.tool_call_id == "latest"]
    assert len(latest) == 1, "the newest edit must survive an oversized call argument"
    assert latest[0].text == "wrote new.py"
    assert sum(len(json.dumps(m.model_dump(), ensure_ascii=False).encode()) for m in result
               if isinstance(m, (AIMessage, ToolMessage))) <= 4096
