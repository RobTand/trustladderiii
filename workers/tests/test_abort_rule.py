"""Abort when a missing capability stops the work (#648), on each role's existing route."""

import errno
from importlib.resources import files
import json
import re
import shutil

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage, ToolMessage
import pytest

from tl3_workers import runtime
from tl3_workers.contracts import schema
from tl3_workers.tools import Worktree

# Local copies of test_workers' replies and ingest request: a test module that imports another test module adds a
# test_to_test edge, which the Gate's graph-impact check refuses.
BRIEF = {
    "kind": "brief", "brief": "Fix the greeting without changing its interface.",
    "goals": [
        {"description": "Return the requested greeting.", "verification": "test"},
        {"description": "Keep the public interface clear.", "verification": "review"},
    ],
    "memory_ids": [],
}
EARLY = {"kind": "early_end", "outcome": "duplicate", "reason": "Issue 2 requests the same fix."}
AGREE = {"verdict": "agree", "objections": []}
DISAGREE = {"verdict": "disagree", "objections": ["The issue also asks for punctuation."]}


@pytest.fixture
def request_doc(tmp_path):
    return {
        "protocol": 1,
        "operation": {"node": 1, "generation": 0, "kind": "ingest"},
        "role": "ingest",
        "model": {"base_url": "http://model.invalid/v1", "name": "fixture-model",
                  "effort": "high", "budget_tokens": 32000},
        "key_file": str(tmp_path / "never-read-this-key"),
        "inputs": {
            "issue": {"number": 1, "title": "Fix greeting", "body": "Return hello.",
                      "author": "requester", "comments": []},
            "repo": {"readme": "A greeting library.", "file_list": ["greeting.py"]},
        },
        "worktree": str(tmp_path),
    }


def abort_rule(role):
    prompt = files("tl3_workers").joinpath("prompts", role + ".md").read_text()
    found = [paragraph for paragraph in prompt.split("\n\n") if paragraph.startswith("Abort when")]
    assert len(found) == 1, f"{role} states its abort rule once"
    return found[0]


def reply_kinds(role):
    kinds = set()
    for variant in schema(role + "-reply")["oneOf"]:
        kind = variant["properties"].get("kind", {})
        kinds.update([kind["const"]] if "const" in kind else kind.get("enum", []))
    return kinds


@pytest.mark.parametrize("role,route", [("maker", ["blocked", "cant_meet"]), ("planner", ["cant_meet"])])
def test_model_abort_rule_names_the_roles_existing_reply(role, route):
    rule = abort_rule(role)
    for capability in ("missing tools", "disk access"):
        assert capability in rule
    named = [kind for kind in sorted(reply_kinds(role)) if re.search(rf"\b{kind}\b", rule)]
    assert named == sorted(route)


@pytest.mark.parametrize("role", ["ingest", "audit", "reviewer", "classifier", "writer", "postmortem"])
def test_roles_without_a_model_route_carry_no_abort_rule(role):
    prompt = files("tl3_workers").joinpath("prompts", role + ".md").read_text()
    assert "Abort when" not in prompt


class ToolModel(FakeMessagesListChatModel):
    """Calls one tool, then answers with the role's final reply."""
    calls: list = []
    call: dict = {"name": "read", "args": {"path": "greeting.py"}}
    final: dict = BRIEF

    def bind_tools(self, tools, **kwargs):
        return self

    def _generate(self, messages, **kwargs):
        self.calls.append(list(messages))
        if len(self.calls) == 1:
            self.responses = [AIMessage(content="", tool_calls=[{**self.call, "id": "1", "type": "tool_call"}])]
        else:
            self.responses = [AIMessage(content=json.dumps(self.final))]
        self.i = 0
        return super()._generate(messages, **kwargs)


def run_ingest(request_doc, monkeypatch, tmp_path, **model_args):
    (tmp_path / "greeting.py").write_text("def greet(): return 'goodbye'\n")
    request_doc["trace"] = {"completion": str(tmp_path / "facts.json")}
    model = ToolModel(responses=[], calls=[], **model_args)
    monkeypatch.setattr(runtime, "create_model", lambda _: model)
    events = []
    error = None
    try:
        runtime.run(request_doc, events.append)
    except Exception as caught:
        error = caught
    return model, events, error


def ended_by_environment(model, events, error, tmp_path):
    assert error is not None, "the run must fail, not continue"
    assert type(error).__name__ == "WorktreeUnavailable"
    assert len(model.calls) == 1, "no model turn follows an environment failure"
    assert not [event for event in events if event.get("event") == "reply"]
    row = json.loads((tmp_path / "facts.json").read_text())
    assert row["outcome"] == "failed"
    assert row["failure"]["class"] == "WorktreeUnavailable"


@pytest.mark.parametrize("code", [errno.EACCES, errno.EIO, errno.EMFILE])
@pytest.mark.parametrize("role", ["ingest", "audit"])
def test_environment_errno_in_a_read_only_role_fails_the_run(request_doc, monkeypatch, tmp_path, code, role):
    request_doc["role"] = request_doc["operation"]["kind"] = role
    if role == "audit":
        request_doc["inputs"]["ingest"] = BRIEF

    def unreadable(self, path: str, offset: int = 0, limit: int = 200, column: int = 0) -> str:
        raise OSError(code, "fixture environment failure", path)

    monkeypatch.setattr(Worktree, "read", unreadable)
    final = AGREE if role == "audit" else BRIEF
    ended_by_environment(*run_ingest(request_doc, monkeypatch, tmp_path, final=final), tmp_path)


def test_a_removed_checkout_fails_the_run(request_doc, monkeypatch, tmp_path):
    checkout = tmp_path / "checkout"
    checkout.mkdir()
    request_doc["worktree"] = str(checkout)
    real = Worktree.read

    def removed(self, path: str, offset: int = 0, limit: int = 200, column: int = 0) -> str:
        shutil.rmtree(checkout)
        return real(self, path, offset, limit, column)

    monkeypatch.setattr(Worktree, "read", removed)
    (checkout / "greeting.py").write_text("x = 1\n")
    request_doc["trace"] = {"completion": str(tmp_path / "facts.json")}
    model = ToolModel(responses=[], calls=[])
    monkeypatch.setattr(runtime, "create_model", lambda _: model)
    events = []
    with pytest.raises(Exception) as caught:
        runtime.run(request_doc, events.append)
    ended_by_environment(model, events, caught.value, tmp_path)


def test_a_missing_file_stays_ordinary_feedback(request_doc, monkeypatch, tmp_path):
    model, events, error = run_ingest(request_doc, monkeypatch, tmp_path,
                                      call={"name": "read", "args": {"path": "absent.py"}})
    assert error is None
    assert events[-1]["reply"]["kind"] == "brief"
    results = [m for m in model.calls[1] if isinstance(m, ToolMessage)]
    assert results[-1].status == "error"
    assert "not found" in results[-1].text


def test_maker_keeps_citable_environment_errors(tmp_path, monkeypatch):
    from tl3_workers.tools import build_tools

    def unreadable(self, path: str, offset: int = 0, limit: int = 200, column: int = 0) -> str:
        raise PermissionError(errno.EACCES, "fixture", path)

    monkeypatch.setattr(Worktree, "read", unreadable)
    with Worktree(str(tmp_path)) as worktree:
        result = build_tools(worktree, {"read": "Read."})[0].invoke({"path": "greeting.py"})
        assert "Tool error id: tool-error-1" in result
        assert worktree.evidence.tool_errors["tool-error-1"]["class"] == "EACCES"


def test_missing_git_ends_a_read_only_diff(tmp_path, monkeypatch):
    from tl3_workers import diff
    from tl3_workers.tools import build_tools

    monkeypatch.setattr(diff, "_GIT", [str(tmp_path / "absent-git"), *diff._GIT[1:]])
    monkeypatch.setattr(diff, "_version", lambda: (2, 43))
    with Worktree(str(tmp_path)) as worktree:
        worktree.diff_checkout, worktree.diff_base = tmp_path, "0" * 40
        worktree.environment_errors_end_run = True
        tool = build_tools(worktree, {"diff": "Diff."})[0]
        with pytest.raises(Exception) as caught:
            tool.invoke({"path": "."})
        assert type(caught.value).__name__ == "WorktreeUnavailable"
