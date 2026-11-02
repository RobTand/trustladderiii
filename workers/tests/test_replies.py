"""A reply wrapped in prose or one code fence still parses; anything ambiguous does not."""
from importlib.resources import files
import json

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage, SystemMessage
import pytest

from tl3_workers import runtime
from tl3_workers.tracing import failure_tags
import test_maker
import test_workers


maker_request = test_maker.maker_request
request_doc = test_workers.request_doc
BRIEF = test_workers.BRIEF
ONLY_JSON = "Reply with one JSON object only: no prose, no code fences."


class Recorder(FakeMessagesListChatModel):
    seen: list = []

    def bind_tools(self, tools, **kwargs):
        return self

    def _generate(self, messages, **kwargs):
        self.seen.append(messages)
        return super()._generate(messages, **kwargs)


def answer(monkeypatch, *texts):
    model = Recorder(responses=[AIMessage(content=text) for text in texts], seen=[])
    monkeypatch.setattr(runtime, "create_model", lambda _: model)
    return model


# Shapes from claude-sonnet-5-5 maker runs on tl3-sandbox#229 and #230.
WRAPPED = [
    "`luhn_valid` is in place and both tests pass.\n\n<br>\n\n```json\n{}\n```",
    "`edit_distance(a, b)` now exists.\n\n- **behavior: met.** The test passes.\n\n```json\n{}\n```",
    "<br>\n{}",
    "```\n{}\n```",
    "Here is my reply: {} Thanks.",
]


@pytest.mark.parametrize("shape", WRAPPED)
def test_wrapped_ingest_reply_parses(request_doc, monkeypatch, shape):
    answer(monkeypatch, shape.replace("{}", json.dumps(BRIEF, indent=1)))
    events = []
    runtime.run(request_doc, events.append)
    assert events[-1]["reply"] == BRIEF


@pytest.mark.parametrize("shape", WRAPPED[:3])
def test_wrapped_maker_reply_parses(maker_request, monkeypatch, shape):
    maker_request["inputs"]["context_size"] = 150_000
    write = test_maker.write_turn()
    model = Recorder(responses=[AIMessage(**write), AIMessage(
        content=shape.replace("{}", json.dumps(test_maker.DONE)))], seen=[])
    monkeypatch.setattr(runtime, "create_model", lambda _: model)
    events = []
    runtime.run(maker_request, events.append)
    assert events[-1]["reply"]["kind"] == "done" and "head" in events[-1]["reply"]


@pytest.mark.parametrize("text,message", [
    (f"First: {json.dumps(BRIEF)}\nSecond: {json.dumps(BRIEF)}", "reply has more than one JSON object"),
    (f"```json\n{json.dumps(BRIEF)}\n```\n```json\n{json.dumps(BRIEF)}\n```", "reply has more than one JSON object"),
    ("I could not finish the brief.", "reply is not one JSON object"),
])
def test_ambiguous_or_missing_reply_is_a_reply_failure(request_doc, monkeypatch, tmp_path, text, message):
    request_doc["memory"] = {"summary_file": str(tmp_path / "summary.json")}
    answer(monkeypatch, text)
    with pytest.raises(ValueError) as raised:
        runtime.run(request_doc, lambda _: None)
    assert failure_tags(raised.value)["failure_message"] == message


@pytest.mark.parametrize("inner", ['{"kind": "brief", "kind": "early_end"}', '{"n": NaN}'])
def test_extracted_json_stays_strict(request_doc, monkeypatch, inner):
    answer(monkeypatch, f"Done.\n```json\n{inner}\n```")
    with pytest.raises(ValueError, match="duplicate JSON key|non-finite JSON number"):
        runtime.run(request_doc, lambda _: None)


@pytest.mark.parametrize("role,request_fixture,text", [
    ("ingest", "request_doc", json.dumps(BRIEF)),
    ("maker", "maker_request", json.dumps({"kind": "cant_meet", "report": "Not possible."})),
])
def test_every_role_prompt_asks_for_json_only(role, request_fixture, text, request, monkeypatch):
    doc = request.getfixturevalue(request_fixture)
    doc.setdefault("inputs", {})["context_size"] = 150_000
    model = answer(monkeypatch, text)
    runtime.run(doc, lambda _: None)
    system = [m for m in model.seen[0] if isinstance(m, SystemMessage)]
    assert system and ONLY_JSON in system[0].text


def test_maker_prompt_asks_for_json_only():
    assert ONLY_JSON in files("tl3_workers").joinpath("prompts", "maker.md").read_text()
