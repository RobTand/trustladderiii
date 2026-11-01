"""Q50: a blocked maker reply cites environment evidence recorded in the same run."""

import errno
import os
from pathlib import Path

from jsonschema import ValidationError
from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
import pytest

from tl3_workers import runtime
from tl3_workers.tools import TOOL_FACTORIES, Worktree, build_tools
from test_maker import maker_request, turn  # noqa: F401

KEY = "c" * 64
OTHER = "d" * 64
TOTALS = {"passed": 0, "failed": 0, "error": 0, "skipped": 0}
INFRASTRUCTURE = {"status": "infrastructure_error", "message": "PrismaBuild failed.",
                  "tests": [], "totals": TOTALS, "note": ""}
COMPLETED = {"status": "completed", "tests": [], "totals": TOTALS, "note": ""}
HELLO = ["test_greeting.py::test_hello"]


def call(tests):
    return {"name": "run_tests", "args": {"tests": tests}, "id": "test", "type": "tool_call"}


def run_maker(maker_request, monkeypatch, results, reply, selectors=None):  # noqa: F811
    """Run the maker through one run_tests call per result, then reply."""
    observed = []
    results = list(results)
    selectors = selectors or [HELLO] * len(results)

    def factory(worktree):
        def run_tests(tests: list[str] | None = None, base: bool = False) -> dict:
            return dict(results.pop(0))
        return run_tests

    class Model(FakeMessagesListChatModel):
        def _generate(self, messages, **kwargs):
            observed.append(messages)
            return super()._generate(messages, **kwargs)

    turns = [AIMessage(**turn(calls=[call(tests)])) for tests in selectors]
    model = Model(responses=[*turns, AIMessage(**turn(reply))])
    monkeypatch.setitem(TOOL_FACTORIES, "run_tests", factory)
    monkeypatch.setattr(runtime, "create_model", lambda request: model)
    events = []
    runtime.run(maker_request, events.append)
    return events[-1]["reply"], observed[-1][-1].text


def test_blocked_cites_an_infrastructure_action_key_from_this_run(maker_request, monkeypatch):  # noqa: F811
    reply = {"kind": "blocked", "evidence": {"action_key": KEY},
             "report": "PrismaBuild could not run the tests."}
    result, _ = run_maker(maker_request, monkeypatch, [{**INFRASTRUCTURE, "action_key": KEY}], reply)
    # Worker code adds the recorded class; the controller cites only that.
    assert result == {**reply, "evidence": {"action_key": KEY, "class": "test_infrastructure"}}


def test_blocked_cites_a_recorded_tool_error(maker_request, monkeypatch):  # noqa: F811
    reply = {"kind": "blocked", "evidence": {"tool_error": "tool-error-1"},
             "report": "The test tool could not submit to PrismaBuild."}
    result, seen = run_maker(maker_request, monkeypatch, [INFRASTRUCTURE], reply)
    assert "tool-error-1" in seen, "the model sees the id it may cite"
    assert result == {**reply, "evidence": {"tool_error": "tool-error-1", "tool": "run_tests",
                                            "class": "test_submission"}}


@pytest.mark.parametrize("results,evidence", [
    ([{**COMPLETED, "action_key": KEY}], {"action_key": KEY}),
    ([{**INFRASTRUCTURE, "action_key": KEY}], {"action_key": OTHER}),
    ([INFRASTRUCTURE], {"tool_error": "tool-error-2"}),
    # The tested code can cut a report short; that is not an environment failure.
    ([{**INFRASTRUCTURE, "action_key": KEY, "origin": "test_run"}], {"action_key": KEY}),
    # A later completed run of the same tests retires the earlier failure.
    ([{**INFRASTRUCTURE, "action_key": KEY}, {**COMPLETED, "action_key": OTHER}], {"action_key": KEY}),
    ([INFRASTRUCTURE, {**COMPLETED, "action_key": OTHER}], {"tool_error": "tool-error-1"}),
])
def test_blocked_without_recorded_evidence_becomes_cant_meet(maker_request, monkeypatch, results, evidence):  # noqa: F811
    reply = {"kind": "blocked", "evidence": evidence, "report": "The sandbox is broken."}
    result, _ = run_maker(maker_request, monkeypatch, results, reply)
    assert result["kind"] == "cant_meet"
    assert "The sandbox is broken." in result["report"]


def test_completed_run_of_other_tests_keeps_the_evidence(maker_request, monkeypatch):  # noqa: F811
    reply = {"kind": "blocked", "evidence": {"action_key": KEY}, "report": "PrismaBuild failed."}
    result, _ = run_maker(maker_request, monkeypatch,
                          [{**INFRASTRUCTURE, "action_key": KEY}, {**COMPLETED, "action_key": OTHER}],
                          reply, selectors=[HELLO, ["test_greeting.py::test_signature"]])
    assert result["kind"] == "blocked"


def test_blocked_needs_structured_evidence(maker_request, monkeypatch):  # noqa: F811
    reply = {"kind": "blocked", "report": "The sandbox is broken."}
    with pytest.raises(ValidationError):
        run_maker(maker_request, monkeypatch, [{**INFRASTRUCTURE, "action_key": KEY}], reply)


@pytest.mark.parametrize("error,recorded", [
    (PermissionError(errno.EACCES, "sandbox refused"), "EACCES"),
    (OSError(errno.EPERM, "not permitted"), "EPERM"),
    (OSError(errno.EROFS, "read-only"), "EROFS"),
    (OSError(errno.ENOSPC, "full"), "ENOSPC"),
    (OSError(errno.EIO, "io"), "EIO"),
    (OSError(errno.EMFILE, "descriptors"), "EMFILE"),
    (NotADirectoryError(errno.ENOTDIR, "not a directory"), None),
    (IsADirectoryError(errno.EISDIR, "directory"), None),
    (FileNotFoundError(errno.ENOENT, "missing"), None),
    (OSError(errno.ENAMETOOLONG, "long"), None),
    (OSError(errno.ELOOP, "link"), None),
    (OSError("no errno"), None),
    (ValueError("Use a relative worktree path"), None),
])
def test_only_environment_errors_get_an_id(tmp_path: Path, monkeypatch, error, recorded):
    def factory(worktree):
        def read(path: str) -> str:
            """Fail like the environment or the model's input."""
            raise error
        return read

    monkeypatch.setitem(TOOL_FACTORIES, "read", factory)
    with Worktree(tmp_path) as worktree:
        tool, = build_tools(worktree, {"read": "Read."})
        message = tool.invoke({"name": "read", "args": {"path": "a.py"}, "id": "read", "type": "tool_call"})
        assert message.status == "error"
        assert ("tool-error-1" in message.text) is (recorded is not None)
        assert worktree.evidence.cites({"tool_error": "tool-error-1"}) is (recorded is not None)


@pytest.mark.parametrize("name,args", [
    ("read", {"path": "a.py/x"}),
    ("read", {"path": "link"}),
    ("read", {"path": "x" * 300}),
    ("list", {"path": "a.py"}),
    ("write", {"path": "a.py/y", "content": "z"}),
])
def test_model_caused_os_errors_get_no_id(tmp_path: Path, name, args):
    # The reviewer's reproductions: each is an OSError the model's own input causes.
    (tmp_path / "a.py").write_text("a\n")
    os.symlink("a.py", tmp_path / "link")
    with Worktree(tmp_path) as worktree:
        tools = {tool.name: tool for tool in build_tools(
            worktree, {"read": "Read.", "list": "List.", "write": "Write."})}
        message = tools[name].invoke({"name": name, "args": args, "id": name, "type": "tool_call"})
        assert message.status == "error"
        assert "tool-error" not in message.text
        assert not worktree.evidence.tool_errors


def test_overlong_paths_are_rejected_without_an_id(tmp_path: Path):
    # A 4.4 KB path would make the test tool's staging fail with ENAMETOOLONG.
    path = "/".join(["d" * 200] * 22) + "/x.py"
    with Worktree(tmp_path) as worktree:
        tool, = build_tools(worktree, {"write": "Write."})
        message = tool.invoke({"name": "write", "args": {"path": path, "content": "x"},
                               "id": "write", "type": "tool_call"})
        assert message.status == "error"
        assert "tool-error" not in message.text
        assert not worktree.evidence.tool_errors
    assert not (tmp_path / ("d" * 200)).exists()


def test_a_walk_too_deep_is_a_plain_tool_error(tmp_path: Path, monkeypatch):
    def factory(worktree):
        def glob(pattern: str) -> dict:
            """Walk a tree deeper than Python's recursion limit."""
            raise RecursionError("maximum recursion depth exceeded")
        return glob

    monkeypatch.setitem(TOOL_FACTORIES, "glob", factory)
    with Worktree(tmp_path) as worktree:
        tool, = build_tools(worktree, {"glob": "Glob."})
        message = tool.invoke({"name": "glob", "args": {"pattern": "**"}, "id": "glob", "type": "tool_call"})
        assert message.status == "error"
        assert "tool-error" not in message.text
        assert not worktree.evidence.tool_errors
