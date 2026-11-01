from dataclasses import asdict
from importlib.resources import files
import json
import os
from pathlib import Path
import subprocess
from threading import Barrier, Event

import httpx
from jsonschema import ValidationError
from langchain_core.callbacks import BaseCallbackHandler
from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
import pytest

from tl3_workers import runtime
from tl3_workers.contracts import validate
from tl3_workers.roles import ROLES
from tl3_workers.roles import maker
from tl3_workers.tools import READ_TOOLS, TOOL_FACTORIES, WRITE_TOOLS
import worker_process
from worker_process import run_real_entrypoint, start_worker


DONE = {
    "kind": "done",
    "self_check": [
        {"goal_id": "greeting", "status": "met", "evidence": "Greeting now returns hello."},
        {"goal_id": "interface", "status": "met", "evidence": "The greet() signature is unchanged."},
    ],
}
NO_USAGE = [None, {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}]


class ToolModel(FakeMessagesListChatModel):
    def bind_tools(self, tools, **kwargs):
        assert {tool.name: tool.description for tool in tools} == ROLES["maker"].tools
        return self


@pytest.fixture(autouse=True)
def tool_capable_process_model(monkeypatch):
    launcher = worker_process.LAUNCH.replace(
        "model = FakeMessagesListChatModel(",
        """class MakerModel(FakeMessagesListChatModel):
    def bind_tools(self, tools, **kwargs):
        from tl3_workers.roles import ROLES
        assert {tool.name: tool.description for tool in tools} == dict(ROLES['maker'].tools)
        return self
model = MakerModel(""",
    )
    monkeypatch.setattr(worker_process, "LAUNCH", launcher)


def turn(reply=None, *, calls=None, input_tokens=100, output_tokens=50):
    return {
        "content": json.dumps(reply) if reply is not None else "",
        "tool_calls": calls or [],
        "usage_metadata": {"input_tokens": input_tokens, "output_tokens": output_tokens,
                           "total_tokens": input_tokens + output_tokens},
    }


def write_turn(path="greeting.py", content="def greet(): return 'hello'\n", **usage):
    return turn(calls=[{"name": "write", "args": {"path": path, "content": content},
                        "id": "write", "type": "tool_call"}], **usage)


def git(root, *args):
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env.update({
        "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_AUTHOR_NAME": "Fixture", "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
        "GIT_COMMITTER_NAME": "Fixture", "GIT_COMMITTER_EMAIL": "fixture@example.invalid",
    })
    return subprocess.run(
        ["git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false", *args],
        cwd=root, env=env, check=True, capture_output=True, text=True,
    ).stdout.strip()


@pytest.fixture
def maker_request(tmp_path):
    root = tmp_path / "repo"
    root.mkdir()
    git(root, "init", "--quiet", "--template=")
    (root / "greeting.py").write_text("def greet(): return 'goodbye'\n")
    git(root, "add", ".")
    git(root, "commit", "--quiet", "-m", "Base")
    return {
        "protocol": 1,
        "operation": {"node": 1, "generation": 0, "kind": "make"},
        "role": "maker",
        "model": {"base_url": "http://model.invalid/v1", "name": "fixture-model",
                  "effort": "high", "budget_tokens": 32000},
        "key_file": str(tmp_path / "unused-key"),
        "worktree": str(root),
        "inputs": {
            "brief": "Return hello without changing greet's interface.",
            "goals": [{"id": "greeting", "description": "Return hello.", "verification": "test"},
                      {"id": "interface", "description": "Keep greet's interface.", "verification": "review"}],
            "plan": {
                "files": ["greeting.py", "test_greeting.py"],
                "checks": [
                    {"name": "hello", "kind": "demonstrating",
                     "tests": ["test_greeting.py::test_hello"], "goal_ids": ["greeting"]},
                    {"name": "signature", "kind": "behavior-preserving",
                     "tests": ["test_greeting.py::test_signature"], "goal_ids": ["interface"]},
                ],
            },
            "attempt": 1, "round": 0,
            "tests": {"image": "sha256:" + "a" * 64,
                      "tools_path": "/mnt/shared/prismabuild-fleet/repo/tools"},
            "context_size": 1000,
        },
    }


def test_maker_declares_only_file_and_pb_tools():
    assert ROLES["maker"].tools.keys() == READ_TOOLS | WRITE_TOOLS | {"run_tests", "recall"}


@pytest.mark.parametrize("reply", [
    {"kind": "needs_files", "files": [{"path": "helper.py", "why": "The shared greeting lives here."}]},
    {"kind": "cant_meet", "report": "The requested interface contradicts the greeting goal."},
    {"kind": "out_of_room"},
])
def test_non_done_replies_through_protocol_do_not_commit(tmp_path, maker_request, reply):
    root = Path(maker_request["worktree"])
    before = git(root, "rev-parse", "HEAD")
    result = start_worker(tmp_path, maker_request, [turn(reply)])
    assert result.exit_code == 0, result.stderr
    assert result.events == [
        {"event": "progress", "unit": "model_turn"},
        {"event": "reply", "role": "maker", "reply": reply},
    ]
    assert git(root, "rev-parse", "HEAD") == before
    assert not Path(maker_request["key_file"]).exists()


def test_done_commits_real_changes_and_reports_head(tmp_path, maker_request):
    root = Path(maker_request["worktree"])
    before = git(root, "rev-parse", "HEAD")
    result = start_worker(tmp_path, maker_request, [
        write_turn(), write_turn("test_greeting.py", "def test_hello(): pass\n"), turn(DONE),
    ])
    assert result.exit_code == 0, result.stderr
    reply = result.events[-1]["reply"]
    assert reply == {**DONE, "head": git(root, "rev-parse", "HEAD")}
    assert reply["head"] != before
    assert git(root, "rev-parse", "HEAD^") == before
    assert git(root, "status", "--porcelain") == ""
    assert git(root, "show", "HEAD:greeting.py") == "def greet(): return 'hello'"
    assert git(root, "show", "HEAD:test_greeting.py") == "def test_hello(): pass"
    assert git(root, "show", "-s", "--format=%an <%ae>%n%cn <%ce>") == (
        "TL3 <tl3@example.invalid>\nTL3 <tl3@example.invalid>"
    )
    assert [event["unit"] for event in result.events[:-1]] == [
        "model_turn", "tool_result", "model_turn", "tool_result", "model_turn",
    ]


def test_commit_ignores_hooks_configs_and_inherited_git_overrides(tmp_path, maker_request):
    root = Path(maker_request["worktree"])
    hooks = root / ".git/hooks"
    hooks.mkdir(exist_ok=True)
    marker = root / "hook-ran"
    for name in ("pre-commit", "prepare-commit-msg", "commit-msg", "post-commit"):
        hook = hooks / name
        hook.write_text("#!/bin/sh\ntouch hook-ran\nexit 42\n")
        hook.chmod(0o755)
    bad_config = tmp_path / "bad-config"
    bad_config.write_text("this is not valid git config [\n")
    result = start_worker(tmp_path, maker_request, [write_turn(), turn(DONE)], env_overrides={
        "GIT_CONFIG_GLOBAL": str(bad_config), "GIT_CONFIG_SYSTEM": str(bad_config),
        "GIT_AUTHOR_NAME": "Wrong author", "GIT_COMMITTER_EMAIL": "wrong@example.invalid",
        "GIT_INDEX_FILE": str(tmp_path / "wrong-index"), "GIT_DIR": str(tmp_path / "wrong-repo"),
        "GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "core.hooksPath",
        "GIT_CONFIG_VALUE_0": str(hooks),
    })
    assert result.exit_code == 0, result.stderr
    assert not marker.exists()
    assert not (tmp_path / "wrong-index").exists()
    assert git(root, "show", "-s", "--format=%an <%ae>%n%cn <%ce>") == (
        "TL3 <tl3@example.invalid>\nTL3 <tl3@example.invalid>"
    )


def test_agent_gitattributes_cannot_run_drivers_at_commit(tmp_path, maker_request):
    root = Path(maker_request["worktree"])
    before = git(root, "rev-parse", "HEAD")
    driver_config = tmp_path / "driver-config"
    for setting in ("filter.probe.clean", "filter.probe.smudge", "filter.probe.process",
                    "diff.probe.command", "diff.probe.textconv", "merge.probe.driver"):
        git(root, "config", "--file", str(driver_config), setting,
            "sh -c 'touch driver-ran; cat'")
    attributes = "*.py filter=probe diff=probe merge=probe\n"
    maker_request["inputs"]["plan"]["files"].append(".gitattributes")
    result = start_worker(tmp_path, maker_request, [
        write_turn(".gitattributes", attributes), write_turn(), turn(DONE),
    ], env_overrides={"GIT_CONFIG_GLOBAL": str(driver_config)})
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]["reply"]["head"] == git(root, "rev-parse", "HEAD")
    assert git(root, "rev-parse", "HEAD^") == before
    assert git(root, "show", "HEAD:.gitattributes") == attributes.strip()
    assert git(root, "show", "HEAD:greeting.py") == "def greet(): return 'hello'"
    assert not (root / "driver-ran").exists()


@pytest.mark.parametrize("turns", [[turn(DONE), turn(DONE)],
    [write_turn(content="def greet(): return 'goodbye'\n"), turn(DONE), turn(DONE)]])
def test_done_without_a_change_reports_cant_meet(tmp_path, maker_request, turns):
    root = Path(maker_request["worktree"])
    before = git(root, "rev-parse", "HEAD")
    result = start_worker(tmp_path, maker_request, turns)
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]['reply'] == {'kind': 'cant_meet', 'report': 'no code change was made'}
    assert git(root, "rev-parse", "HEAD") == before


def test_done_with_an_unmet_goal_is_rejected_before_commit(tmp_path, maker_request):
    root = Path(maker_request["worktree"])
    before = git(root, "rev-parse", "HEAD")
    reply = {"kind": "done", "self_check": [DONE["self_check"][0],
             {"goal_id": "interface", "status": "not_met", "why": "Signature check is still failing."}]}
    result = start_worker(tmp_path, maker_request, [write_turn(), turn(reply)])
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]["reply"]["kind"] == "cant_meet"
    assert git(root, "rev-parse", "HEAD") == before
    assert git(root, "diff", "--cached", "--name-only") == ""
    assert (root / "greeting.py").read_text() == "def greet(): return 'hello'\n"


@pytest.mark.parametrize("reply", [
    {**DONE, "head": "b" * 40},
    {**DONE, "self_check": [DONE["self_check"][0]]},
    {**DONE, "self_check": [DONE["self_check"][0]] * 2},
    {**DONE, "self_check": [DONE["self_check"][0], {"goal_id": "unknown", "status": "met", "evidence": "x"}]},
    {**DONE, "self_check": [{"goal_id": "greeting", "status": "met"}, DONE["self_check"][1]]},
    {**DONE, "self_check": [{"goal_id": "greeting", "status": "not_met"}, DONE["self_check"][1]]},
    {**DONE, "self_check": [{"goal_id": "greeting", "status": "met", "evidence": "\ud800"}, DONE["self_check"][1]]},
    {"kind": "cant_meet", "report": " "},
    {"kind": "needs_files", "files": []},
    {"kind": "needs_files", "files": [{"path": "../outside.py", "why": "x"}]},
    {"kind": "needs_files", "files": [{"path": ".git/config", "why": "x"}]},
    {"kind": "out_of_room", "extra": True},
])
def test_invalid_replies_never_commit(tmp_path, maker_request, reply):
    root = Path(maker_request["worktree"])
    before = git(root, "rev-parse", "HEAD")
    result = start_worker(tmp_path, maker_request, [write_turn(), turn(reply)])
    assert result.exit_code == 1
    assert all(event["event"] != "reply" for event in result.events)
    assert git(root, "rev-parse", "HEAD") == before
    assert (root / "greeting.py").read_text() == "def greet(): return 'hello'\n"


@pytest.mark.parametrize("input_tokens,output_tokens", [(850, 0), (840, 11), (900, 30)])
def test_usage_cutoff_stops_before_pending_tool_dispatch(tmp_path, maker_request, input_tokens, output_tokens):
    root = Path(maker_request["worktree"])
    before = git(root, "rev-parse", "HEAD")
    result = start_worker(tmp_path, maker_request, [
        write_turn("must-not-exist", input_tokens=input_tokens, output_tokens=output_tokens), turn(DONE),
    ])
    assert result.exit_code == 0, result.stderr
    assert result.events == [
        {"event": "progress", "unit": "model_turn"},
        {"event": "reply", "role": "maker", "reply": {"kind": "out_of_room"}},
    ]
    assert not (root / "must-not-exist").exists()
    assert git(root, "rev-parse", "HEAD") == before


def test_cutoff_after_edit_keeps_changes_uncommitted(tmp_path, maker_request):
    root = Path(maker_request["worktree"])
    before = git(root, "rev-parse", "HEAD")
    result = start_worker(tmp_path, maker_request, [write_turn(), turn(DONE, input_tokens=810, output_tokens=50)])
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]["reply"] == {"kind": "out_of_room"}
    assert git(root, "rev-parse", "HEAD") == before
    assert (root / "greeting.py").read_text() == "def greet(): return 'hello'\n"


def test_usage_is_latest_context_not_cumulative_tokens(tmp_path, maker_request):
    result = start_worker(tmp_path, maker_request, [
        write_turn(input_tokens=600, output_tokens=100), turn(DONE, input_tokens=749, output_tokens=100),
    ])
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]["reply"]["kind"] == "done"


@pytest.mark.parametrize("usage", NO_USAGE, ids=["missing", "zero"])
def test_usage_fallback_counts_system_prompt_before_tools(tmp_path, maker_request, usage):
    response = write_turn("must-not-exist")
    response["usage_metadata"] = usage
    # The input alone is below the cutoff. The system prompt and schema exceed it.
    assert len(json.dumps(maker_request["inputs"], ensure_ascii=False)) < 2550
    result = start_worker(tmp_path, maker_request, [response, turn(DONE)])
    assert result.exit_code == 0, result.stderr
    assert not (Path(maker_request["worktree"]) / "must-not-exist").exists()
    assert result.events == [
        {"event": "progress", "unit": "model_turn"},
        {"event": "reply", "role": "maker", "reply": {"kind": "out_of_room"}},
    ]


@pytest.mark.parametrize("usage", NO_USAGE, ids=["missing", "zero"])
def test_usage_fallback_allows_work_below_cutoff(tmp_path, maker_request, usage):
    maker_request["inputs"]["context_size"] = 100_000
    turns = [write_turn(), turn(DONE)]
    for response in turns:
        response["usage_metadata"] = usage
    result = start_worker(tmp_path, maker_request, turns)
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]["reply"] == {
        **DONE, "head": git(maker_request["worktree"], "rev-parse", "HEAD"),
    }


@pytest.mark.parametrize("usage", NO_USAGE, ids=["missing", "zero"])
@pytest.mark.parametrize("source", ["history", "tool_result"])
def test_usage_fallback_includes_history_and_tool_results(maker_request, monkeypatch, usage, source):
    maker_request["inputs"]["context_size"] = 10_000
    root = Path(maker_request["worktree"])
    before = git(root, "rev-parse", "HEAD")
    if source == "history":
        first = write_turn(content="x" * 30_000)
    else:
        def factory(worktree):
            def run_tests() -> dict:
                return {"status": "completed", "details": "x" * 30_000}
            return run_tests
        monkeypatch.setitem(TOOL_FACTORIES, "run_tests", factory)
        first = turn(calls=[{"name": "run_tests", "args": {}, "id": "test", "type": "tool_call"}])
    pending = write_turn("must-not-exist")
    pending["usage_metadata"] = usage
    model = ToolModel(responses=[AIMessage(**first), AIMessage(**pending), AIMessage(**turn(DONE))])
    monkeypatch.setattr(runtime, "create_model", lambda request: model)
    events = []
    runtime.run(maker_request, events.append)
    assert events == [
        {"event": "progress", "unit": "model_turn"},
        {"event": "progress", "unit": "tool_result"},
        {"event": "progress", "unit": "model_turn"},
        {"event": "reply", "role": "maker", "reply": {"kind": "out_of_room"}},
    ]
    assert not (root / "must-not-exist").exists()
    assert git(root, "rev-parse", "HEAD") == before


@pytest.mark.parametrize("usage", NO_USAGE, ids=["missing", "zero"])
@pytest.mark.parametrize("characters,expected", [(2547, False), (2548, True), (2550, True)])
def test_usage_estimate_rounds_up_before_applying_85_percent(usage, characters, expected):
    message = AIMessage(content="x", usage_metadata=usage)
    assert maker.out_of_room(message, 1000, characters - 1) is expected


def test_done_schema_is_selected_by_kind(maker_request, monkeypatch):
    original = maker.schema

    def reordered(name):
        document = original(name)
        return {**document, "oneOf": list(reversed(document["oneOf"]))}

    monkeypatch.setattr(maker, "schema", reordered)
    model = ToolModel(responses=[AIMessage(**write_turn()), AIMessage(**turn(DONE))])
    monkeypatch.setattr(runtime, "create_model", lambda request: model)
    events = []
    runtime.run(maker_request, events.append)
    assert events[-1]["reply"] == {
        **DONE, "head": git(maker_request["worktree"], "rev-parse", "HEAD"),
    }


@pytest.mark.parametrize("names", [("write", "run_tests"), ("edit", "run_tests"), ("write", "edit")])
def test_mutations_and_tests_from_one_turn_do_not_overlap(maker_request, monkeypatch, names):
    dispatched = Barrier(2)
    dispatches = []
    entered = {name: Event() for name in names}
    trace = []
    snapshots = []
    root = Path(maker_request["worktree"])

    class DispatchBarrier(BaseCallbackHandler):
        raise_error = True

        def on_tool_start(self, serialized, input_str, **kwargs):
            dispatches.append(serialized["name"])
            dispatched.wait(timeout=5)

    build_agent = runtime.build_agent

    def with_barrier(*args, **kwargs):
        return build_agent(*args, **kwargs).with_config(callbacks=[DispatchBarrier()])

    monkeypatch.setattr(runtime, "build_agent", with_barrier)

    def observe(name, operation):
        trace.append((name, "start"))
        entered[name].set()
        other = next(item for item in names if item != name)
        entered[other].wait(timeout=0.2)
        result = operation()
        trace.append((name, "end"))
        return result

    def write_factory(worktree):
        def write(path: str, content: str) -> str:
            return observe("write", lambda: worktree.write(path, content))
        return write

    def edit_factory(worktree):
        def edit(path: str, old: str, new: str) -> str:
            return observe("edit", lambda: worktree.edit(path, old, new))
        return edit

    def test_factory(worktree):
        def run_tests() -> dict:
            def snapshot():
                snapshots.append((root / "greeting.py").read_text())
                return {"status": "completed", "totals": {"passed": 1}}
            return observe("run_tests", snapshot)
        return run_tests

    for name, factory in (("write", write_factory), ("edit", edit_factory), ("run_tests", test_factory)):
        monkeypatch.setitem(TOOL_FACTORIES, name, factory)
    arguments = {
        "write": {"path": "greeting.py", "content": "def greet(): return 'hello'\n"},
        "edit": {"path": "greeting.py", "old": "return", "new": "return  "},
        "run_tests": {},
    }
    response = {"kind": "cant_meet", "report": "More work is needed."}
    model = ToolModel(responses=[AIMessage(**turn(calls=[
        {"name": name, "args": arguments[name], "id": name, "type": "tool_call"} for name in names
    ])), AIMessage(**turn(response))])
    monkeypatch.setattr(runtime, "create_model", lambda request: model)
    events = []
    runtime.run(maker_request, events.append)
    assert sorted(dispatches) == sorted(names)
    assert [phase for _, phase in trace] == ["start", "end", "start", "end"]
    assert trace[0][0] == trace[1][0]
    assert trace[2][0] == trace[3][0]
    assert trace[0][0] != trace[2][0]
    if "run_tests" in names:
        assert len(snapshots) == 1
        assert snapshots[0] in {
            "def greet(): return 'goodbye'\n", "def greet(): return 'hello'\n",
            "def greet(): return   'goodbye'\n",
        }
    assert events[-1]["reply"] == response
    assert [event["unit"] for event in events[:-1]].count("tool_result") == 2


@pytest.mark.parametrize("input_tokens,output_tokens,kind", [
    (749, 100, "done"), (750, 100, "out_of_room"),
    (0, 0, "out_of_room"), (None, None, "out_of_room"),
])
def test_proxy_stream_requests_usage_and_serial_tools_and_uses_usage(
    maker_request, monkeypatch, input_tokens, output_tokens, kind,
):
    requests = []
    root = Path(maker_request["worktree"])
    before = git(root, "rev-parse", "HEAD")
    Path(maker_request["key_file"]).write_text("fixture-key")

    def respond(request):
        body = json.loads(request.content)
        requests.append(body)
        assert request.url == "http://model.invalid/v1/chat/completions"
        assert body["stream"] is True
        assert body["stream_options"] == {"include_usage": True}
        assert body["parallel_tool_calls"] is False
        assert {tool["function"]["name"] for tool in body["tools"]} == set(ROLES["maker"].tools)
        if len(requests) == 1:
            delta = {"role": "assistant", "tool_calls": [{
                "index": 0, "id": "write", "type": "function",
                "function": {"name": "write", "arguments": json.dumps({
                    "path": "greeting.py", "content": "def greet(): return 'hello'\n",
                })},
            }]}
            finish_reason = "tool_calls"
            usage = {"prompt_tokens": 100, "completion_tokens": 50, "total_tokens": 150}
        else:
            assert len(requests) == 2
            assert any(message["role"] == "tool" for message in body["messages"])
            delta = {"role": "assistant", "content": json.dumps(DONE)}
            finish_reason = "stop"
            usage = None if input_tokens is None else {
                "prompt_tokens": input_tokens, "completion_tokens": output_tokens,
                "total_tokens": input_tokens + output_tokens,
            }
        chunk = {"id": "fixture-completion", "object": "chat.completion.chunk", "created": 0,
                 "model": "fixture-model"}
        frames = [
            {**chunk, "choices": [{"index": 0, "delta": delta, "finish_reason": None}]},
            {**chunk, "choices": [{"index": 0, "delta": {}, "finish_reason": finish_reason}]},
        ]
        if usage is not None:
            frames.append({**chunk, "choices": [], "usage": usage})
        content = "".join("data: " + json.dumps(frame) + "\n\n" for frame in frames) + "data: [DONE]\n\n"
        return httpx.Response(200, headers={"content-type": "text/event-stream"}, content=content)

    proxy = runtime.ProxyChatOpenAI
    with httpx.Client(transport=httpx.MockTransport(respond)) as client:
        monkeypatch.setattr(runtime, "ProxyChatOpenAI", lambda **kwargs: proxy(http_client=client, **kwargs))
        events = []
        runtime.run(maker_request, events.append)
    assert len(requests) == 2
    assert [event["unit"] for event in events[:-1]] == ["model_turn", "tool_result", "model_turn"]
    assert events[-1]["reply"]["kind"] == kind
    if kind == "done":
        assert events[-1]["reply"] == {**DONE, "head": git(root, "rev-parse", "HEAD")}
        assert git(root, "rev-parse", "HEAD^") == before
    else:
        assert git(root, "rev-parse", "HEAD") == before
    assert (root / "greeting.py").read_text() == "def greet(): return 'hello'\n"


@pytest.mark.parametrize("invalid", ["duplicate-goal", "unknown-check-goal", "unknown-feedback-goal", "uncovered-goal"])
def test_goal_references_are_checked_before_tool_calls(tmp_path, maker_request, invalid):
    inputs = maker_request["inputs"]
    if invalid == "duplicate-goal":
        inputs["goals"].append(inputs["goals"][0])
    elif invalid == "unknown-check-goal":
        inputs["plan"]["checks"][0]["goal_ids"] = ["unknown"]
    elif invalid == "unknown-feedback-goal":
        inputs["feedback"] = [{"kind": "review_finding", "report": "x", "goal_ids": ["unknown"]}]
    else:
        inputs["plan"]["checks"] = inputs["plan"]["checks"][:1]
    result = start_worker(tmp_path, maker_request, [write_turn("must-not-exist"), turn(DONE)])
    assert result.exit_code == 1
    assert result.events == []
    assert not (Path(maker_request["worktree"]) / "must-not-exist").exists()


def test_footprint_is_left_for_the_controller_to_check(tmp_path, maker_request):
    result = start_worker(tmp_path, maker_request, [write_turn("outside-plan.py"), turn(DONE)])
    assert result.exit_code == 0, result.stderr
    root = Path(maker_request["worktree"])
    assert git(root, "show", "HEAD:outside-plan.py") == "def greet(): return 'hello'"


@pytest.mark.parametrize("field,value", [
    ("round", 2), ("attempt", 0), ("attempt", 4), ("context_size", 0),
    ("tests", {"image": "mutable:latest", "tools_path": "/tools"}),
    ("tests", {"image": "sha256:" + "a" * 64, "tools_path": "relative"}),
    ("round", 1),
])
def test_invalid_inputs_fail_at_request_boundary(maker_request, field, value):
    maker_request["inputs"][field] = value
    result = run_real_entrypoint(maker_request)
    assert result.returncode == 1
    assert result.stdout == ""
    assert result.stderr.strip() == "tl3_workers: request failed"


def test_check_base_schema_accepts_only_commit_ids(maker_request):
    from jsonschema import Draft202012Validator
    from tl3_workers.contracts import schema

    validator = Draft202012Validator(schema("maker-input"))
    inputs = maker_request["inputs"]
    for value in ("a" * 40, "b" * 64):
        assert validator.is_valid({**inputs, "check_base_sha": value})
    for value in ("HEAD", "-main", "A" * 40, "a" * 39, None):
        assert not validator.is_valid({**inputs, "check_base_sha": value})


def test_check_base_reaches_maker_test_tool(maker_request):
    from tl3_workers.tools import Worktree

    base = git(maker_request["worktree"], "rev-parse", "HEAD")
    maker_request["inputs"]["check_base_sha"] = base
    response = {"kind": "cant_meet", "report": "Fixture stops after tool binding."}
    with Worktree(maker_request["worktree"]) as tree:
        maker.run(ToolModel(responses=[AIMessage(**turn(response))]), maker_request, tree, lambda event: None)
        assert getattr(tree, "check_base_sha", None) == base


def test_plan_check_kinds_reach_maker_test_tool(maker_request):
    from tl3_workers.tools import Worktree

    response = {"kind": "cant_meet", "report": "Fixture stops after tool binding."}
    with Worktree(maker_request["worktree"]) as tree:
        maker.run(ToolModel(responses=[AIMessage(**turn(response))]), maker_request, tree, lambda event: None)
        assert getattr(tree, "test_checks", None) == maker_request["inputs"]["plan"]["checks"]


def test_prompt_distinguishes_excused_regressions_and_unverified_evidence():
    prompt = " ".join(files("tl3_workers").joinpath("prompts", "maker.md").read_text().split())
    assert "pre-existing only for tests named in regression checks" in prompt
    assert "fails-on-base means this test also failed or errored on base, but no regression check names it." in prompt
    assert "fails-on-base does not excuse demonstrating, reproducing or behavior-preserving checks" in prompt
    assert "Never edit unrelated code, pins or tests to make it pass" in prompt
    assert "Name it and both runs in your self-check; it is not a reason for cant_meet" in prompt
    assert "For a goal verified by review, give the change and your account" in prompt
    assert "Name evidence your tools cannot reach as unverified; review decides" in prompt
    assert "A failing check your change caused is never unverified." in prompt


def test_fix_round_inputs_and_test_settings_reach_their_destinations(tmp_path, maker_request, monkeypatch):
    observed = []
    settings = []
    checkouts = []
    inputs = maker_request["inputs"]
    inputs.update({"attempt": 2, "round": 1, "feedback": [
        {"kind": "failed_check", "report": "test_hello failed.", "goal_ids": ["greeting"]},
        {"kind": "review_finding", "report": "Keep greet's signature.", "goal_ids": ["interface"]},
    ]})
    inputs["tests"].update(cpus=2, mem_gb=3, quiet_seconds=60, exec_scratch=True)

    def factory(worktree):
        settings.append(asdict(worktree.test_settings))
        checkouts.append(getattr(worktree, "test_checkout", None))

        def run_tests(tests: list[str] | None = None) -> dict:
            assert tests == ["test_greeting.py::test_hello"]
            return {"status": "completed", "totals": {"passed": 1}, "action_key": "fixture-key"}
        return run_tests

    class InspectModel(FakeMessagesListChatModel):
        def bind_tools(self, tools, **kwargs):
            assert {tool.name: tool.description for tool in tools} == ROLES["maker"].tools
            return self

        def _generate(self, messages, **kwargs):
            observed.append(messages)
            return super()._generate(messages, **kwargs)

    response = {"kind": "cant_meet", "report": "Further work is needed."}
    fake = InspectModel(responses=[AIMessage(**turn(calls=[{
        "name": "run_tests", "args": {"tests": ["test_greeting.py::test_hello"]},
        "id": "test", "type": "tool_call",
    }])), AIMessage(**turn(response))])
    monkeypatch.setitem(TOOL_FACTORIES, "run_tests", factory)
    monkeypatch.setattr(runtime, "create_model", lambda request: fake)
    events = []
    runtime.run(maker_request, events.append)
    assert settings == [inputs["tests"]]
    # The PrismaBuild look ties a declared wait to this checkout's history.
    assert checkouts == [Path(maker_request["worktree"])]
    assert json.loads(observed[0][1].text) == inputs
    system = observed[0][0].text
    assert system.startswith(files("tl3_workers").joinpath("prompts/maker.md").read_text())
    assert maker_request["key_file"] not in system
    assert "fixture-key" in observed[-1][-1].text
    assert events[-1]["reply"] == response
    validate("maker-input", inputs)


@pytest.mark.parametrize("name,args", [
    ("execute", {"command": "touch forbidden"}), ("task", {"description": "delegate"}),
    ("read_file", {"file_path": "/outside"}), ("fetch", {"url": "http://network.invalid"}),
])
def test_maker_rejects_shell_and_other_undeclared_tools(tmp_path, maker_request, name, args):
    response = {"kind": "cant_meet", "report": "The forbidden operation was refused."}
    result = start_worker(tmp_path, maker_request, [turn(calls=[
        {"name": name, "args": args, "id": "bad", "type": "tool_call"},
    ]), turn(response)])
    assert result.exit_code == 0, result.stderr
    assert result.events == [
        {"event": "progress", "unit": "model_turn"},
        {"event": "progress", "unit": "tool_result"},
        {"event": "progress", "unit": "model_turn"},
        {"event": "reply", "role": "maker", "reply": response},
    ]
    assert not (Path(maker_request["worktree"]) / "forbidden").exists()


@pytest.mark.parametrize('fit_tokens,linked,expected', [(100000, False, True), (1, False, False), (100000, True, False)])
def test_requested_file_fit_is_computed_by_worker_code(tmp_path, maker_request, fit_tokens, linked, expected):
    maker_request['inputs']['fit_tokens'] = fit_tokens
    root = Path(maker_request['worktree'])
    if linked:
        (root / 'helper.py').symlink_to(root / 'greeting.py')
    result = start_worker(tmp_path, maker_request, [turn({'kind': 'needs_files', 'files': [{'path': 'helper.py', 'why': 'A shared helper is needed.'}]})])
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]['reply']['fits'] is expected


def test_model_cannot_claim_that_requested_files_fit(tmp_path, maker_request):
    maker_request['inputs']['fit_tokens'] = 100000
    result = start_worker(tmp_path, maker_request, [turn({'kind': 'needs_files', 'files': [{'path': 'helper.py', 'why': 'Needed.'}], 'fits': True})])
    assert result.exit_code == 1
    assert all(event['event'] != 'reply' for event in result.events)


@pytest.mark.parametrize('resume_requested', [False, True])
def test_adopted_fix_round_without_session_starts_fresh(tmp_path, maker_request, resume_requested):
    root = Path(maker_request['worktree'])
    (root / 'author.txt').write_text('Keep the author commit.\n')
    git(root, 'add', '.')
    git(root, 'commit', '-m', 'Author proposal')
    before = git(root, 'rev-parse', 'HEAD')
    session = tmp_path / 'missing-session.json'
    maker_request['session_file'] = str(session)
    maker_request['resume_session'] = resume_requested
    maker_request['inputs']['round'] = 1
    maker_request['inputs']['feedback'] = [{'kind': 'failed_check', 'report': 'Greeting still fails.', 'goal_ids': ['greeting']}]
    result = start_worker(tmp_path, maker_request, [write_turn(), turn(DONE)])
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]['reply']['kind'] == 'done'
    assert git(root, 'rev-parse', 'HEAD^') == before
    assert (root / 'author.txt').read_text() == 'Keep the author commit.\n'
    history = json.loads(session.read_text())
    assert len([message for message in history if message['type'] == 'human']) == 1
    assert 'Greeting still fails.' in json.dumps(history)


@pytest.mark.parametrize('session_path_supplied', [False, True])
def test_accepted_maker_session_missing_fails_closed(tmp_path, maker_request, session_path_supplied):
    root = Path(maker_request['worktree'])
    before = git(root, 'rev-parse', 'HEAD')
    session = tmp_path / 'missing-accepted-session.json'
    if session_path_supplied:
        maker_request['session_file'] = str(session)
    maker_request['resume_session'] = True
    maker_request['last_accepted_generation'] = 0
    maker_request['operation']['generation'] = 1
    maker_request['inputs']['round'] = 1
    maker_request['inputs']['feedback'] = [{'kind': 'failed_check', 'report': 'Greeting still fails.', 'goal_ids': ['greeting']}]
    result = start_worker(tmp_path, maker_request, [write_turn(), turn(DONE)])
    assert result.exit_code == 1, result.stderr
    assert result.stderr.strip() == 'tl3_workers: run failed'
    assert result.events == []
    assert git(root, 'rev-parse', 'HEAD') == before
    assert (root / 'greeting.py').read_text() == "def greet(): return 'goodbye'\n"
    assert not session.exists()
    assert not session.with_suffix('.head').exists()


def test_retry_after_crash_after_commit_reuses_the_round_commit(maker_request, monkeypatch, tmp_path):
    root = Path(maker_request['worktree'])
    base = git(root, 'rev-parse', 'HEAD')
    session = tmp_path / 'session.json'
    maker_request['session_file'] = str(session)
    model = ToolModel(responses=[AIMessage(**write_turn()), AIMessage(**turn(DONE))])
    monkeypatch.setattr(runtime, 'create_model', lambda request: model)
    commit = maker._commit

    def crash_after_commit(*args):
        commit(*args)
        raise RuntimeError('crash after commit, before successful exit')

    monkeypatch.setattr(maker, '_commit', crash_after_commit)
    with pytest.raises(RuntimeError, match='crash after commit'):
        runtime.run(maker_request, lambda event: None)
    head = git(root, 'rev-parse', 'HEAD')
    assert head != base
    assert not session.exists(), 'a raised run must not save its history'
    monkeypatch.setattr(maker, '_commit', commit)
    monkeypatch.setattr(runtime, 'create_model', lambda request: ToolModel(responses=[AIMessage(**turn(DONE))]))
    events = []
    runtime.run(maker_request, events.append)
    assert events[-1]['reply'] == {**DONE, 'head': head}
    assert git(root, 'rev-parse', 'HEAD^') == base
    assert git(root, 'rev-parse', 'HEAD') == head
    assert session.exists()


@pytest.mark.parametrize('failure', ['schema', 'stream'])
def test_failed_run_preserves_the_last_clean_session(maker_request, monkeypatch, tmp_path, failure):
    maker_request['inputs']['context_size'] = 150000
    session = tmp_path / 'session.json'
    maker_request['session_file'] = str(session)
    reply = {'kind': 'cant_meet', 'report': 'Need another approach.'}
    monkeypatch.setattr(runtime, 'create_model', lambda request: ToolModel(responses=[AIMessage(**turn(reply))]))
    runtime.run(maker_request, lambda event: None)
    clean = session.read_bytes()
    maker_request['resume_session'] = True
    if failure == 'schema':
        monkeypatch.setattr(runtime, 'create_model', lambda request: ToolModel(responses=[AIMessage(content='invalid json')]))
    else:
        class BrokenAgent:
            def stream(self, *args, **kwargs):
                yield {'model': {'messages': [AIMessage(content='partial history')]}}
                raise ValueError('stream failed')
        monkeypatch.setattr(runtime, 'build_agent', lambda *args, **kwargs: BrokenAgent())
    with pytest.raises(ValueError):
        runtime.run(maker_request, lambda event: None)
    assert session.read_bytes() == clean
    assert not session.with_suffix('.tmp').exists()


def test_actionable_check_feedback_matches_the_input_contract(maker_request):
    maker_request['inputs']['round'] = 1
    maker_request['inputs']['feedback'] = [{
        'kind': 'failed_check', 'report': 'The greeting assertion failed and secret.py is outside the plan.',
        'goal_ids': ['greeting'],
        'failed_tests': [{'test': 'test_greeting.py::test_hello', 'message': "assert 'goodbye' == 'hello'"}],
        'outside_paths': ['secret.py'],
        'ci_checks': [{'name': 'greeting', 'summary': 'Greeting assertion failed.', 'url': 'https://example.invalid/check/1'}],
    }]
    validate('maker-input', maker_request['inputs'])


@pytest.mark.parametrize('moved_head', [False, True])
@pytest.mark.parametrize('new_edits', [False, True])
def test_rebase_needs_changes_revisit_uses_its_own_checkpoint(
        maker_request, monkeypatch, tmp_path, moved_head, new_edits):
    root = Path(maker_request['worktree'])
    maker_request['session_file'] = str(tmp_path / 'session.json')
    maker_request['inputs']['context_size'] = 150000
    monkeypatch.setattr(runtime, 'create_model', lambda request: ToolModel(
        responses=[AIMessage(**write_turn()), AIMessage(**turn(DONE))]))
    runtime.run(maker_request, lambda event: None)
    if moved_head:
        # A rewritten upstream history need not retain the first visit's starting commit.
        rebased = git(root, 'commit-tree', 'HEAD^{tree}', '-m', 'Rebased history')
        git(root, 'reset', '--hard', rebased)
    before = git(root, 'rev-parse', 'HEAD')
    maker_request['last_accepted_generation'] = maker_request['operation']['generation']
    maker_request['operation']['generation'] += 1
    maker_request['resume_session'] = True
    turns = [write_turn(content="def greet(): return 'hello again'\n")] if new_edits else []
    monkeypatch.setattr(runtime, 'create_model', lambda request: ToolModel(
        responses=[AIMessage(**message) for message in [*turns, turn(DONE)]]))
    if new_edits:
        events = []
        runtime.run(maker_request, events.append)
        assert events[-1]['reply']['kind'] == 'done'
        assert git(root, 'rev-parse', 'HEAD^') == before
    else:
        events = []
        runtime.run(maker_request, events.append)
        assert events[-1]['reply'] == {'kind': 'cant_meet', 'report': 'no code change was made'}
        assert git(root, 'rev-parse', 'HEAD') == before
    checkpoint = Path(maker_request['session_file']).with_suffix('.head')
    assert json.loads(checkpoint.read_text()) == {
        'generation': maker_request['operation']['generation'], 'head': before,
    }
    assert list(tmp_path.glob('*.head')) == [checkpoint], 'visits replace one bounded checkpoint'


@pytest.mark.parametrize('existing', [False, True])
@pytest.mark.parametrize('usage_cutoff', [False, True])
def test_out_of_room_never_saves_a_conversation(maker_request, monkeypatch, tmp_path, existing, usage_cutoff):
    session = tmp_path / 'session.json'
    maker_request['session_file'] = str(session)
    clean = b'[]'
    if existing:
        session.write_bytes(clean)
    message = turn({'kind': 'out_of_room'}, input_tokens=900 if usage_cutoff else 100)
    monkeypatch.setattr(runtime, 'create_model', lambda request: ToolModel(responses=[AIMessage(**message)]))
    events = []
    runtime.run(maker_request, events.append)
    assert events[-1]['reply']['kind'] == 'out_of_room'
    assert session.read_bytes() == clean if existing else not session.exists()


def test_ci_feedback_summary_has_a_character_cap(maker_request):
    maker_request['inputs']['round'] = 1
    maker_request['inputs']['feedback'] = [{
        'kind': 'failed_check', 'report': 'CI failed.', 'goal_ids': ['greeting'],
        'ci_checks': [{'name': 'Rust', 'summary': 'x' * 1000, 'url': None}],
    }]
    validate('maker-input', maker_request['inputs'])
    maker_request['inputs']['feedback'][0]['ci_checks'][0]['summary'] += 'x'
    with pytest.raises(ValidationError):
        validate('maker-input', maker_request['inputs'])


@pytest.mark.parametrize('failure', ['killed', 'session-save'])
@pytest.mark.parametrize('failures', [1, 2])
@pytest.mark.parametrize('fix_round', [False, True])
def test_new_generation_recovers_commit_before_reply(tmp_path, maker_request, monkeypatch, failure, failures, fix_round):
    root = Path(maker_request['worktree'])
    base = git(root, 'rev-parse', 'HEAD')
    maker_request['session_file'] = str(tmp_path / 'session.json')
    maker_request['inputs']['context_size'] = 150000
    launcher = worker_process.LAUNCH
    if fix_round:
        accepted = start_worker(tmp_path, maker_request, [write_turn(), turn(DONE)])
        assert accepted.exit_code == 0, accepted.stderr
        maker_request['last_accepted_generation'] = maker_request['operation']['generation']
        maker_request['operation']['generation'] += 1
        maker_request['inputs']['round'] = 1
        maker_request['inputs']['feedback'] = [{'kind': 'failed_check', 'report': 'Repair the greeting.', 'goal_ids': ['greeting']}]
        maker_request['resume_session'] = True
    visit_base = git(root, 'rev-parse', 'HEAD')
    visit_generation = maker_request['operation']['generation']
    injection = """
from tl3_workers.roles import maker
commit = maker._commit
save = maker.save
def crash_commit(*args):
    commit(*args)
    import os, signal
    os.kill(os.getpid(), signal.SIGKILL)
def fail_save(path, value):
    if path.suffix == '.json':
        raise OSError('injected session save failure')
    save(path, value)
"""
    injection += ("maker._commit = crash_commit\n" if failure == 'killed'
                  else "maker.save = fail_save\n")
    monkeypatch.setattr(worker_process, 'LAUNCH', launcher.replace('def no_network', injection + '\ndef no_network'))
    for index in range(failures):
        turns = [write_turn(content="def greet(): return 'hello fixed'\n")] if index == 0 else []
        failed = start_worker(tmp_path, maker_request, [*turns, turn(DONE)])
        assert failed.exit_code == (-9 if failure == 'killed' else 1), failed.stderr
        assert all(event['event'] != 'reply' for event in failed.events)
        head = git(root, 'rev-parse', 'HEAD')
        assert head != visit_base
        assert git(root, 'rev-list', '--count', f'{visit_base}..HEAD') == '1'
        maker_request['operation']['generation'] += 1
    monkeypatch.setattr(worker_process, 'LAUNCH', launcher)
    recovered = start_worker(tmp_path, maker_request, [turn(DONE)])
    assert recovered.exit_code == 0, recovered.stderr
    assert recovered.events[-1]['reply'] == {**DONE, 'head': head}
    assert git(root, 'rev-list', '--count', f'{base}..HEAD') == str(2 if fix_round else 1)
    assert git(root, 'rev-parse', 'HEAD') == head
    checkpoint = Path(maker_request['session_file']).with_suffix('.head')
    assert json.loads(checkpoint.read_text()) == {'generation': visit_generation, 'head': visit_base}


@pytest.mark.parametrize('paths', [['outside.py'] * 101, ['é' * 4097]])
def test_outside_path_feedback_schema_is_bounded(maker_request, paths):
    maker_request['inputs']['feedback'] = [{
        'kind': 'failed_check', 'report': 'Files outside the footprint.',
        'goal_ids': ['greeting'], 'outside_paths': paths,
    }]
    # Use distinct paths so duplicate rejection cannot substitute for the item cap.
    if len(paths) > 1:
        maker_request['inputs']['feedback'][0]['outside_paths'] = [f'outside-{i}.py' for i in range(101)]
    with pytest.raises(ValidationError):
        validate('maker-input', maker_request['inputs'])


@pytest.fixture
def review_fix(maker_request):
    inputs = maker_request['inputs']
    inputs.update(round=1, reviewed_sha='a' * 40, feedback=[
        {'kind': 'review_finding', 'id': 'review-4-0', 'reviewed_sha': 'a' * 40,
         'report': 'Greeting fails for an empty name.', 'goal_ids': ['greeting']},
        {'kind': 'review_finding', 'id': 'review-4-1', 'reviewed_sha': 'a' * 40,
         'report': 'Describe the signature.', 'goal_ids': ['interface']},
        {'kind': 'failed_check', 'report': 'Existing test failed.', 'goal_ids': ['greeting']},
        {'kind': 'review_finding', 'id': 'old-review', 'reviewed_sha': 'b' * 40,
         'report': 'An earlier attempt failed.', 'goal_ids': ['greeting']},
    ])
    inputs['current_review_findings'] = inputs['feedback'][:2]
    reply = {**DONE, 'reproductions': [
        {'finding_id': 'review-4-0', 'tests': ['test_greeting.py::test_empty_name']},
        {'finding_id': 'review-4-1', 'reason': 'This is a documentation correction.'},
    ]}
    return inputs, reply


def test_review_fix_contract_accepts_named_tests_and_reasons(review_fix):
    inputs, reply = review_fix
    from tl3_workers.contracts import schema
    from jsonschema import Draft202012Validator
    assert Draft202012Validator(schema('maker-input')).is_valid(inputs), 'reviewed findings need identities'
    assert Draft202012Validator(schema('maker-reply')).is_valid({**reply, 'head': 'c' * 40}), 'done must carry reproductions'
    maker._check_reply(inputs, reply)


@pytest.mark.parametrize('invalid', ['omitted', 'partial', 'duplicate', 'unknown', 'old', 'selector'])
def test_review_fix_must_cover_current_findings_exactly_once(review_fix, invalid):
    inputs, reply = review_fix
    if invalid == 'omitted':
        reply.pop('reproductions')
    elif invalid == 'partial':
        reply['reproductions'].pop()
    elif invalid == 'duplicate':
        reply['reproductions'].append(reply['reproductions'][0])
    elif invalid == 'unknown':
        reply['reproductions'][0]['finding_id'] = 'invented'
    elif invalid == 'old':
        reply['reproductions'][0]['finding_id'] = 'old-review'
    else:
        reply['reproductions'][0]['tests'] = ['test_greeting.py']
    with pytest.raises(ValueError, match='reproduc|finding|pytest'):
        maker._check_reply(inputs, reply)


def test_failed_checks_do_not_require_new_tests(maker_request):
    inputs = maker_request['inputs']
    inputs.update(round=1, feedback=[{
        'kind': 'failed_check', 'report': 'CI test failed.', 'goal_ids': ['greeting'],
    }])
    maker._check_reply(inputs, DONE)


def test_standing_rule_feedback_needs_no_goal_citation(maker_request):
    maker_request['inputs']['round'] = 1
    maker_request['inputs']['feedback'] = [{
        'kind': 'review_finding', 'report': 'Protect the test report stream.', 'goal_ids': [],
    }]
    validate('maker-input', maker_request['inputs'])
    maker._check_inputs(maker_request['inputs'])


def test_maker_uses_controller_selection_of_current_findings(maker_request):
    inputs = maker_request['inputs']
    selected = {'kind': 'review_finding', 'id': 'chosen', 'reviewed_sha': 'a' * 40,
                'report': 'Fix it', 'goal_ids': ['greeting']}
    inputs.update(round=0, feedback=[selected], current_review_findings=[selected])
    reply = {**DONE, 'reproductions': [{'finding_id': 'chosen', 'reason': 'Review this manually.'}]}
    maker._check_reply(inputs, reply)


@pytest.mark.parametrize('response', ['cant_meet', 'done', 'repair'])
def test_no_change_done_gets_one_feedback_turn(tmp_path, maker_request, monkeypatch, response):
    from langchain_core.messages import HumanMessage

    root = Path(maker_request['worktree'])
    before = git(root, 'rev-parse', 'HEAD')
    maker_request['inputs']['attempt'] = 3
    maker_request['model']['name'] = 'claude-fable-5-1'
    maker_request['inputs']['context_size'] = 150000
    maker_request['session_file'] = str(tmp_path / 'session.json')
    report = 'Cannot satisfy test_greeting.py::test_hello without changing the protected test.'
    final = {'kind': 'cant_meet', 'report': report} if response == 'cant_meet' else DONE
    turns = [turn(DONE), *([write_turn()] if response == 'repair' else []), turn(final)]
    seen = []
    class RecordingModel(ToolModel):
        def _generate(self, messages, *args, **kwargs):
            seen.append(list(messages))
            return super()._generate(messages, *args, **kwargs)
    model = RecordingModel(responses=[AIMessage(**message) for message in turns])
    monkeypatch.setattr(runtime, 'create_model', lambda _: model)
    events = []
    try:
        runtime.run(maker_request, events.append)
    except ValueError:
        pytest.fail('no-change done crashed instead of returning feedback to the maker')
    feedback = 'done requires code changes; if no code change can satisfy the failing checks, reply cant_meet with a report that names them'
    assert any(isinstance(message, HumanMessage) and message.content == feedback for message in seen[1])
    reply = events[-1]['reply']
    if response == 'repair':
        assert reply == {**DONE, 'head': git(root, 'rev-parse', 'HEAD')}
        assert git(root, 'rev-parse', 'HEAD^') == before
    else:
        assert reply == {'kind': 'cant_meet', 'report': report if response == 'cant_meet' else 'no code change was made'}
        assert git(root, 'rev-parse', 'HEAD') == before
    assert len(seen) == len(turns)
    saved = json.loads(Path(maker_request['session_file']).read_text())
    assert any(message['data']['content'] == feedback for message in saved)


def test_no_change_done_recovers_through_protocol(tmp_path, maker_request):
    before = git(maker_request['worktree'], 'rev-parse', 'HEAD')
    maker_request['inputs']['attempt'] = 3
    maker_request['model']['name'] = 'claude-fable-5-1'
    reply = {'kind': 'cant_meet', 'report': 'The named regression check does not exist at base.'}
    result = start_worker(tmp_path, maker_request, [turn(DONE), turn(reply)])
    assert result.exit_code == 0, result.stderr
    assert result.events[-1] == {'event': 'reply', 'role': 'maker', 'reply': reply}
    assert git(maker_request['worktree'], 'rev-parse', 'HEAD') == before


def test_conflict_resolution_commits_merge_even_when_keeping_leaf_tree(maker_request):
    from tl3_workers.roles import maker

    root = Path(maker_request['worktree'])
    base = git(root, 'rev-parse', 'HEAD')
    (root / 'greeting.py').write_text("def greet(): return 'hello'\n")
    git(root, 'add', '.')
    git(root, 'commit', '-m', 'Leaf')
    leaf = git(root, 'rev-parse', 'HEAD')
    branch = git(root, 'symbolic-ref', '--short', 'HEAD')
    git(root, 'checkout', '-b', 'upstream', base)
    (root / 'greeting.py').write_text("def greet(): return 'bonjour'\n")
    git(root, 'add', '.')
    git(root, 'commit', '-m', 'Main')
    target = git(root, 'rev-parse', 'HEAD')
    git(root, 'checkout', branch)
    with pytest.raises(subprocess.CalledProcessError):
        git(root, 'merge', '--no-commit', target)
    (root / 'greeting.py').write_text("def greet(): return 'hello'\n")
    head = maker._commit(maker_request, leaf)
    assert head and head != leaf, 'a resolution retaining the leaf tree must still finish the merge'
    assert git(root, 'rev-parse', 'HEAD^1') == leaf
    assert git(root, 'rev-parse', 'HEAD^2') == target
    assert git(root, 'status', '--porcelain') == ''


def pending_conflict(request):
    root = Path(request['worktree'])
    base = git(root, 'rev-parse', 'HEAD')
    (root / 'greeting.py').write_text("def greet(): return 'leaf'\n")
    git(root, 'add', '.')
    git(root, 'commit', '-m', 'Leaf')
    leaf = git(root, 'rev-parse', 'HEAD')
    branch = git(root, 'symbolic-ref', '--short', 'HEAD')
    git(root, 'checkout', '-b', 'upstream', base)
    (root / 'greeting.py').write_text("def greet(): return 'upstream'\n")
    git(root, 'add', '.')
    git(root, 'commit', '-m', 'Main')
    target = git(root, 'rev-parse', 'HEAD')
    git(root, 'checkout', branch)
    with pytest.raises(subprocess.CalledProcessError):
        git(root, 'merge', '--no-commit', target)
    return root, leaf, target


@pytest.mark.parametrize('failure', ['markers', 'aborted', 'committed-markers'])
def test_pending_merge_done_requires_resolution_and_original_target(maker_request, monkeypatch, failure):
    root, leaf, target = pending_conflict(maker_request)
    class InvalidResolution(ToolModel):
        def _generate(self, *args, **kwargs):
            if failure == 'aborted':
                git(root, 'merge', '--abort')
                (root / 'greeting.py').write_text("def greet(): return 'edited on old base'\n")
            elif failure == 'committed-markers':
                git(root, 'add', '.')
                git(root, 'commit', '-m', 'Unresolved markers')
            return super()._generate(*args, **kwargs)
    monkeypatch.setattr(runtime, 'create_model', lambda _: InvalidResolution(responses=[AIMessage(**turn(DONE))]))
    events = []
    runtime.run(maker_request, events.append)
    reply = events[-1]['reply']
    assert reply['kind'] == 'cant_meet', 'an unresolved or aborted merge must never report done'
    assert 'merge' in reply['report'].lower()
    if failure == 'markers':
        assert git(root, 'rev-parse', 'HEAD') == leaf, 'worker must not commit unresolved conflict markers'
        assert git(root, 'rev-parse', 'MERGE_HEAD') == target


def test_merge_target_survives_crash_after_an_aborted_resolution(maker_request, monkeypatch, tmp_path):
    root, leaf, target = pending_conflict(maker_request)
    maker_request['session_file'] = str(tmp_path / 'session.json')
    class AbortAndCrash(ToolModel):
        def _generate(self, *args, **kwargs):
            git(root, 'merge', '--abort')
            (root / 'greeting.py').write_text("def greet(): return 'old base edit'\n")
            raise RuntimeError('crash after abort')
    monkeypatch.setattr(runtime, 'create_model', lambda _: AbortAndCrash(responses=[]))
    with pytest.raises(RuntimeError, match='crash after abort'):
        runtime.run(maker_request, lambda _: None)
    monkeypatch.setattr(runtime, 'create_model', lambda _: ToolModel(responses=[AIMessage(**turn(DONE))]))
    events = []
    runtime.run(maker_request, events.append)
    assert events[-1]['reply']['kind'] == 'cant_meet', 'retry must retain the original pending merge target'
    assert git(root, 'rev-parse', 'HEAD') == leaf


@pytest.mark.parametrize('documentation', [
    'Title\n=========\n',
    'Marker examples\n<<<<<<< example\n>>>>>>> example\n',
])
def test_resolved_merge_ignores_unconflicted_documentation(maker_request, monkeypatch, documentation):
    root = Path(maker_request['worktree'])
    (root / 'README.rst').write_text(documentation)
    git(root, 'add', 'README.rst')
    git(root, 'commit', '-m', 'Document the project')
    root, leaf, target = pending_conflict(maker_request)
    class Resolved(ToolModel):
        def _generate(self, *args, **kwargs):
            (root / 'greeting.py').write_text("def greet(): return 'resolved'\n")
            git(root, 'add', 'greeting.py')
            return super()._generate(*args, **kwargs)
    monkeypatch.setattr(runtime, 'create_model', lambda _: Resolved(responses=[AIMessage(**turn(DONE))]))
    events = []
    runtime.run(maker_request, events.append)
    assert events[-1]['reply']['kind'] == 'done', 'unconflicted documentation must not block a resolved merge'
    assert git(root, 'rev-parse', 'HEAD^1') == leaf
    assert git(root, 'rev-parse', 'HEAD^2') == target
    assert (root / 'README.rst').read_text() == documentation


@pytest.mark.parametrize('legacy_checkpoint', [False, True])
def test_original_conflict_files_survive_staging_and_restart(maker_request, monkeypatch, tmp_path, legacy_checkpoint):
    root, leaf, target = pending_conflict(maker_request)
    session = tmp_path / 'session.json'
    maker_request['session_file'] = str(session)
    class StageAndCrash(ToolModel):
        def _generate(self, *args, **kwargs):
            (root / 'greeting.py').write_text('<<<<<<< leftover\nnot resolved\n')
            git(root, 'add', 'greeting.py')
            raise RuntimeError('crash after staging')
    monkeypatch.setattr(runtime, 'create_model', lambda _: StageAndCrash(responses=[]))
    with pytest.raises(RuntimeError, match='crash after staging'):
        runtime.run(maker_request, lambda _: None)
    checkpoint = session.with_suffix('.head')
    saved = json.loads(checkpoint.read_text())
    assert saved.get('merge_files') == ['greeting.py'], 'the checkpoint must retain the originally unmerged files'
    if legacy_checkpoint:
        del saved['merge_files']
        checkpoint.write_text(json.dumps(saved))
    monkeypatch.setattr(runtime, 'create_model', lambda _: ToolModel(responses=[AIMessage(**turn(DONE))]))
    events = []
    runtime.run(maker_request, events.append)
    assert events[-1]['reply']['kind'] == 'cant_meet', 'staging must not hide leftover markers on restart'
    assert git(root, 'rev-parse', 'HEAD') == leaf
