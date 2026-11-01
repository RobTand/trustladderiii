"""Budget checks use the real graph and confined tools, without a provider."""
import json

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage, ToolMessage
import pytest

from tl3_workers import runtime
from tl3_workers.contracts import validate
from test_workers import AGREE, BRIEF


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
    assert len(seen) <= 5, "at most four exploration turns and one final answer"
    assert len(seen) * (120_000 if large_input else 1000) <= 300_000
    assert events[-1]["reply"] == reply
    validate(role + "-reply", events[-1]["reply"])
    # Retained tool results must stay bounded as the model keeps exploring.
    assert max(sum(len(m.text.encode()) for m in messages if isinstance(m, ToolMessage))
               for messages in seen) <= 32_768


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
    runtime.run({"role": role, "worktree": str(tmp_path), "inputs": {}}, events.append)
    results = [m for m in observed[-1] if isinstance(m, ToolMessage)]
    assert {m.tool_call_id for m in results} == {str(i) for i in range(5)}
    assert sum(len(m.text.encode()) for m in results) <= 32_768
    assert any("clipped" in m.text for m in observed[-1] if m.type == "human")
    assert events[-1]["reply"] == reply
