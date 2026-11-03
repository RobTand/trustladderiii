"""The maker's retrospective: one tool-free turn after its accepted reply (#648)."""

from copy import deepcopy
from dataclasses import asdict, replace
import json
from types import SimpleNamespace

import httpx
from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage, HumanMessage
import openai
import pytest

from tl3_workers import runtime
from tl3_workers.roles import maker
import test_maker_budget
import test_tracing

# pytest collects shared fixtures under these module-level names.
maker_request = test_maker_budget.maker_request
receiver = test_tracing.receiver
exporters = test_tracing.exporters

CANT_MEET = {"kind": "cant_meet", "report": "The plan needs a file it does not declare."}
RETRO = {"slowed_by": ["Three reads of greeting.py repeated the same lines."],
         "missing": ["The brief did not name the test runner image."],
         "checks": ["A lint for unused imports would have caught my first edit."],
         "noise": []}
WORK_TOKENS, RETRO_TOKENS = 100, 7_777


class RetroModel(FakeMessagesListChatModel):
    """Answers the maker's first turn with its final reply, then the retro turn."""
    calls: list = []
    offered: set = set()
    final_reply: dict = CANT_MEET
    work_turns: int = 1
    retro: object = RETRO

    def bind_tools(self, tools, **kwargs):
        self.offered = {tool.name for tool in tools}
        return self

    def _generate(self, messages, **kwargs):
        index = len(self.calls)
        self.calls.append((deepcopy(messages), set(self.offered)))
        self.offered = set()
        if index < self.work_turns - 1:
            message = AIMessage(content="", tool_calls=[{"name": "read", "args": {"path": "greeting.py"},
                                                         "id": str(index), "type": "tool_call"}],
                                usage_metadata={"input_tokens": WORK_TOKENS, "output_tokens": 1,
                                                "total_tokens": WORK_TOKENS + 1})
        elif index == self.work_turns - 1:
            message = AIMessage(content=json.dumps(self.final_reply), usage_metadata={
                "input_tokens": WORK_TOKENS, "output_tokens": 1, "total_tokens": WORK_TOKENS + 1})
        else:
            if isinstance(self.retro, BaseException):
                raise self.retro
            text = self.retro if isinstance(self.retro, str) else json.dumps(self.retro)
            message = AIMessage(content=text, usage_metadata={
                "input_tokens": RETRO_TOKENS, "output_tokens": 50, "total_tokens": RETRO_TOKENS + 50,
                "input_token_details": {"cache_read": 7_000}})
        self.responses = [message]
        self.i = 0
        return super()._generate(messages, **kwargs)


def install(monkeypatch, request, tmp_path, **model_args):
    request["inputs"]["context_size"] = 150_000
    request["trace"] = {"completion": str(tmp_path / "facts.json")}
    model = RetroModel(responses=[], calls=[], **model_args)
    monkeypatch.setattr(runtime, "create_model", lambda _: model)
    return model


def facts(tmp_path):
    return json.loads((tmp_path / "facts.json").read_text())


def test_retro_follows_the_accepted_reply_without_tools(maker_request, monkeypatch, tmp_path):
    model = install(monkeypatch, maker_request, tmp_path)
    events = []
    runtime.run(maker_request, events.append)
    assert events[-1]["reply"] == CANT_MEET
    assert len(model.calls) == 2, "one retro turn follows the accepted reply"
    messages, offered = model.calls[1]
    assert offered == set(), "the retro turn is tool-free"
    assert any(isinstance(m, AIMessage) and json.loads(m.text) == CANT_MEET for m in messages), \
        "the retro sees the run it looks back on"
    assert isinstance(messages[-1], HumanMessage)
    assert "retro" in messages[-1].text.lower()
    row = facts(tmp_path)
    assert row["outcome"] == "succeeded"
    assert row["retro"]["status"] == "recorded"
    assert row["retro"]["result"] == RETRO


def test_retro_usage_is_outside_the_budget_and_the_run_totals(maker_request, monkeypatch, tmp_path):
    # Two working turns exhaust a one-turn allowance, so the reply carries the budget diagnostic.
    budget = SimpleNamespace(**{**asdict(maker.ROLE.exploration_budget), "tool_turns": 1,
                                "input_tokens": 6_000_000, "finalization_turns": 0})
    monkeypatch.setattr(maker, "ROLE", replace(maker.ROLE, exploration_budget=budget))
    model = install(monkeypatch, maker_request, tmp_path, work_turns=2)
    events = []
    runtime.run(maker_request, events.append)
    reply = events[-1]["reply"]
    assert len(model.calls) == 3, "the retro still runs after the allowance is spent"
    assert reply["budget_exhaustion"]["input_tokens"]["used"] == 2 * WORK_TOKENS
    row = facts(tmp_path)
    assert (row["input_tokens"], row["output_tokens"]) == (2 * WORK_TOKENS, 2)
    assert row["usage_complete"] is True
    assert row["retro"]["input_tokens"] == RETRO_TOKENS
    assert row["retro"]["cached_tokens"] == 7_000
    assert row["retro"]["output_tokens"] == 50


def _timeout():
    return openai.APITimeoutError(request=httpx.Request("POST", "http://model.invalid/v1"))


@pytest.mark.parametrize("retro,status", [
    (RuntimeError("proxy text that must not be kept"), "failed"),
    (_timeout(), "timeout"),
    ("I learned a lot.", "invalid"),
    ({**RETRO, "noise": ["x" * 241]}, "invalid"),
    ({**RETRO, "extra": []}, "invalid"),
])
def test_a_failing_retro_leaves_the_outcome_unchanged(maker_request, monkeypatch, tmp_path, retro, status):
    install(monkeypatch, maker_request, tmp_path, retro=retro)
    events = []
    runtime.run(maker_request, events.append)
    assert events[-1] == {"event": "reply", "role": "maker", "reply": CANT_MEET}
    row = facts(tmp_path)
    assert row["outcome"] == "succeeded"
    assert row["failure"] is None
    assert row["input_tokens"] == WORK_TOKENS
    assert row["retro"]["status"] == status
    assert "result" not in row["retro"]
    assert "proxy text" not in (tmp_path / "facts.json").read_text()


def test_retro_is_a_separate_span_on_the_maker_trace(maker_request, monkeypatch, tmp_path, exporters, receiver):
    uri, packets = receiver
    maker_request["mlflow"] = {"tracking_uri": uri, "experiment": "tl3-test"}
    install(monkeypatch, maker_request, tmp_path)
    runtime.run(maker_request, lambda event: None)
    for child in exporters:
        assert child.wait(timeout=15) == 0
    spans = test_tracing.exported_spans(packets)
    root = next(span for span in spans if span.parent_id is None)
    retro = next(span for span in spans if span.name == "tl3.maker_retro")
    assert retro.parent_id == root.span_id
    assert retro.outputs["status"] == "recorded"
    assert retro.outputs["result"] == RETRO
    retro_models = [span for span in spans if span.parent_id == retro.span_id
                    and span.span_type == "CHAT_MODEL"]
    assert len(retro_models) == 1
    assert root.get_attribute("mlflow.traceTag.retro") == "recorded"
    assert root.get_attribute("mlflow.traceTag.input_tokens") == str(WORK_TOKENS)
    assert root.get_attribute("mlflow.traceTag.retro_input_tokens") == str(RETRO_TOKENS)
    assert root.outputs == CANT_MEET
