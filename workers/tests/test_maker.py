from dataclasses import asdict
from importlib.resources import files
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
from threading import Barrier, Event

import httpx
from jsonschema import ValidationError
from langchain_core.callbacks import BaseCallbackHandler
from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage, ToolMessage
import pytest
from message_invariants import agent_calls, assert_tool_pairs, is_retro

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


def turn(reply=None, *, calls=None, input_tokens=100, output_tokens=50, reasoning=None):
    usage = {"input_tokens": input_tokens, "output_tokens": output_tokens,
             "total_tokens": input_tokens + output_tokens}
    if reasoning is not None:
        usage["output_token_details"] = {"reasoning": reasoning}
    return {
        "content": json.dumps(reply) if reply is not None else "",
        "tool_calls": calls or [],
        "usage_metadata": usage,
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


@pytest.fixture(scope="session")
def cache_tokenizer(tmp_path_factory):
    from tiktoken.load import read_file_cached
    from tl3_workers.forecast import TOKENIZER_URL, TOKENIZER_SHA256

    data = read_file_cached(TOKENIZER_URL, TOKENIZER_SHA256)
    cache = tmp_path_factory.mktemp("maker-tokenizer")
    (cache / hashlib.sha1(TOKENIZER_URL.encode()).hexdigest()).write_bytes(data)
    return str(cache)


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
    assert ROLES["maker"].tools.keys() == READ_TOOLS | WRITE_TOOLS | {"run_tests", "run_generator", "recall"}


def test_done_hands_ordinary_demonstration_to_checking(maker_request, tmp_path):
    root = Path(maker_request["worktree"])
    base = git(root, "rev-parse", "HEAD")
    maker_request["inputs"]["check_base_sha"] = base
    result = start_worker(tmp_path, maker_request, [write_turn(), turn(DONE)])
    assert result.exit_code == 0, result.stderr
    reply = result.events[-1]["reply"]
    assert reply["kind"] == "done", reply
    assert reply["checking_handoff"] == {
        "base_sha": base, "head_sha": reply["head"],
        "head_tree": git(root, "rev-parse", reply["head"] + "^{tree}"),
        "status": "pending", "owner": "checking",
        "checks": [{"name": "hello", "tests": ["test_greeting.py::test_hello"],
                    "mode": "head_tests_on_base"}],
    }
    validate("maker-reply", reply)


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
    {**DONE, "checking_handoff": {"status": "passed"}},
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


def reasoning_usage(input_tokens, output_tokens, reasoning):
    usage = {"input_tokens": input_tokens, "output_tokens": output_tokens,
             "total_tokens": input_tokens + output_tokens}
    if reasoning is not None:
        usage["output_token_details"] = {"reasoning": reasoning}
    return usage


def read_call(path="greeting.py"):
    return [{"name": "read", "args": {"path": path}, "id": "read", "type": "tool_call"}]


@pytest.mark.parametrize("reasoning", [None, 39_900], ids=["no-details", "reasoning-details"])
@pytest.mark.parametrize("input_tokens,output_tokens", [
    (84_073, 36_544),  # issue #355: the 59.7-minute turn of request 87
    (88_000, 40_000),  # the next turn: 128,000 of 150,000 by the old sum, 88,300 resent
])
def test_reasoning_tokens_do_not_fill_the_context(input_tokens, output_tokens, reasoning):
    message = AIMessage(content="", tool_calls=read_call(),
                        usage_metadata=reasoning_usage(input_tokens, output_tokens, reasoning))
    assert maker.out_of_room(message, 150_000, 1) is False


@pytest.mark.parametrize("with_details", [True, False], ids=["reasoning-details", "no-details"])
def test_visible_output_still_counts_toward_the_context(with_details):
    # 48,000 visible tokens at three characters each, resent with the next prompt.
    visible = "x" * 144_000
    reasoning = 2_000 if with_details else None
    message = AIMessage(content=visible, usage_metadata=reasoning_usage(80_000, 50_000, reasoning))
    assert maker.out_of_room(message, 150_000, 1) is True


@pytest.mark.parametrize("output_tokens,expected", [(47_499, False), (47_500, True)])
def test_reasoning_details_leave_exactly_the_visible_tokens(output_tokens, expected):
    message = AIMessage(content="ok", usage_metadata=reasoning_usage(80_000, 50_000, 50_000 - output_tokens))
    assert maker.out_of_room(message, 150_000, 1) is expected


def test_maker_continues_past_a_turn_that_only_reasoned_long(tmp_path, maker_request):
    root = Path(maker_request["worktree"])
    result = start_worker(tmp_path, maker_request, [
        write_turn(input_tokens=700, output_tokens=400, reasoning=390),
        turn(DONE, input_tokens=720, output_tokens=300, reasoning=250),
    ])
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]["reply"] == {**DONE, "head": git(root, "rev-parse", "HEAD")}


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


# Without reasoning details, the reply's characters bound the visible output.
DONE_TOKENS = (len(json.dumps(DONE)) + 2) // 3


@pytest.mark.parametrize("input_tokens,output_tokens,kind", [
    (849 - DONE_TOKENS, 100, "done"), (850 - DONE_TOKENS, 100, "out_of_room"),
    (0, 0, "out_of_room"), (None, None, "out_of_room"),
])
def test_proxy_stream_requests_usage_and_serial_tools_and_uses_usage(
    maker_request, monkeypatch, input_tokens, output_tokens, kind,
):
    requests, retros = [], []
    root = Path(maker_request["worktree"])
    before = git(root, "rev-parse", "HEAD")
    Path(maker_request["key_file"]).write_text("fixture-key")

    def respond(request):
        body = json.loads(request.content)
        if is_retro(body):
            # The retro turn (#648) is tool-free, so it sends no tool options.
            retros.append(body)
            assert "tools" not in body and "parallel_tool_calls" not in body
            frame = {"id": "fixture-retro", "object": "chat.completion.chunk", "created": 0,
                     "model": "fixture-model", "choices": [{"index": 0, "finish_reason": "stop", "delta": {
                         "role": "assistant", "content": json.dumps(dict.fromkeys(
                             ("slowed_by", "missing", "checks", "noise"), []))}}]}
            return httpx.Response(200, headers={"content-type": "text/event-stream"},
                                  content="data: " + json.dumps(frame) + "\n\ndata: [DONE]\n\n")
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
    assert len(retros) == (0 if kind == "out_of_room" else 1), "a full context ends without a retro"
    # The retro (#648) reports its model turn before the reply, except after out_of_room.
    retro_turn = [] if kind == "out_of_room" else ["model_turn"]
    assert [event["unit"] for event in events[:-1]] == ["model_turn", "tool_result", "model_turn", *retro_turn]
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


def test_prompt_tells_history_records_from_open_feedback():
    # #316: superseded findings arrive as one-line records with resolved_round.
    prompt = " ".join(files("tl3_workers").joinpath("prompts", "maker.md").read_text().split())
    assert "A feedback item with resolved_round is a one-line record of history that a later review or check superseded; it is not a task." in prompt
    assert "The full-text items without resolved_round are the open ones to fix." in prompt


def test_prompt_requires_reds_the_harness_accepts():
    # #340: a module-level import of a new name errors on the base, and a flaky
    # test can pass there; neither demonstrates the change.
    prompt = " ".join(files("tl3_workers").joinpath("prompts", "maker.md").read_text().split())
    assert "must fail by assertion on the base" in prompt
    assert "Import names the change adds inside the test body, or reach them through their module" in prompt
    assert "a test failure, not a collection error" in prompt
    assert "Prefer new test files where the plan allows" in prompt
    assert "force the bad interleaving or timing deterministically with injected clocks, events or barriers" in prompt
    assert "head's tests overlaid on the base implementation" in prompt
    assert "Ordinary demonstrating proof is controller-owned and remains pending until Checking" in prompt
    assert "A plain base pass neither supplies nor disproves that RED" in prompt
    assert "Confirm the base outcome with run_tests and its base comparison before replying done" not in prompt


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
    assert "fixture-key" in next(message.text for message in reversed(observed[-1])
                                 if isinstance(message, ToolMessage))
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
def test_requested_file_fit_is_computed_by_worker_code(tmp_path, maker_request, cache_tokenizer, fit_tokens, linked, expected):
    maker_request['inputs']['forecast'] = {
        'fit_tokens': fit_tokens, 'context_tokens': 150000, 'overhead_tokens': {'reserve': 32000},
    }
    root = Path(maker_request['worktree'])
    if linked:
        (root / 'helper.py').symlink_to(root / 'greeting.py')
    result = start_worker(tmp_path, maker_request, [turn({'kind': 'needs_files', 'files': [{'path': 'helper.py', 'why': 'A shared helper is needed.'}]})],
                          env_overrides={'TIKTOKEN_CACHE_DIR': cache_tokenizer})
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]['reply']['fits'] is expected


@pytest.mark.parametrize('content_source', ['brief', 'file'])
@pytest.mark.parametrize('cache', ['available', 'disabled', 'empty', 'corrupt'])
def test_requested_file_fit_counts_tokens_with_byte_fallback(tmp_path, maker_request, cache_tokenizer, content_source, cache):
    maker_request['inputs']['forecast'] = {
        'fit_tokens': 100000, 'context_tokens': 150000, 'overhead_tokens': {'reserve': 32000},
    }
    if content_source == 'brief':
        maker_request['inputs']['brief'] = 'token ' * 18000
    else:
        root = Path(maker_request['worktree'])
        for name in ('greeting.py', 'helper.py'):
            (root / name).write_text('token ' * 6000)
    directory = cache_tokenizer
    if cache == 'disabled':
        directory = ''
    elif cache in ('empty', 'corrupt'):
        directory = tmp_path / 'unavailable-tokenizer'
        directory.mkdir()
        if cache == 'corrupt':
            (directory / 'fb374d419588a4632f3f557e76b4b70aebbca790').write_bytes(b'bad cache')
        directory = str(directory)
    result = start_worker(tmp_path, maker_request, [turn({
        'kind': 'needs_files', 'files': [{'path': 'helper.py', 'why': 'Needed.'}],
    })], env_overrides={'TIKTOKEN_CACHE_DIR': directory})
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]['reply']['fits'] is (cache == 'available')
    assert result.stderr == ''


def test_model_cannot_claim_that_requested_files_fit(tmp_path, maker_request):
    maker_request['inputs']['forecast'] = {
        'fit_tokens': 100000, 'context_tokens': 150000, 'overhead_tokens': {'reserve': 32000},
    }
    result = start_worker(tmp_path, maker_request, [turn({'kind': 'needs_files', 'files': [{'path': 'helper.py', 'why': 'Needed.'}], 'fits': True})])
    assert result.exit_code == 1
    assert all(event['event'] != 'reply' for event in result.events)


def requested_fit(maker_request, paths):
    from tl3_workers.tools import Worktree

    response = {'kind': 'needs_files', 'files': [{'path': path, 'why': 'Needed.'} for path in paths]}
    with Worktree(maker_request['worktree']) as tree:
        return maker.run(ToolModel(responses=[AIMessage(**turn(response))]),
                         maker_request, tree, lambda event: None)['fits']


@pytest.mark.parametrize('extra_tokens,expected', [(0, True), (4000, True), (18000, False)])
def test_pq_2629_plan_and_requested_file_fit(maker_request, cache_tokenizer, monkeypatch, extra_tokens, expected):
    from tl3_workers.forecast import Forecaster

    monkeypatch.setenv('TIKTOKEN_CACHE_DIR', cache_tokenizer)
    root = Path(maker_request['worktree'])
    monkeypatch.chdir(root)
    inputs = maker_request['inputs']
    config = {'fit_tokens': 100000, 'context_tokens': 150000,
              'overhead_tokens': {'prompt': 5000, 'making': 20000}}
    inputs['forecast'] = config
    inputs['brief'] = 'x\n' * 1643
    # Model PQ #2629's eight files: two ranged files and six small files.
    costs = dict(zip([f'plan_{i}.py' for i in range(8)], [16384, 16384, 4000, 4000, 4000, 4000, 4000, 2232]))
    for index, (path, tokens) in enumerate(costs.items()):
        (root / path).write_text('large ' * 14000 if index < 2 else 'x\n' * (tokens // 2))
    inputs['plan']['files'] = list(costs)
    # The remaining input brought the old maker JSON count to about 25k.
    inputs['goals'][0]['description'] = ' token' * 21000
    inputs.update(round=1, feedback=[{'kind': 'failed_check', 'report': 'Fix the check.', 'goal_ids': ['greeting']}])
    forecast = Forecaster(config).measure(inputs['brief'], inputs['plan']['files'])
    assert forecast['brief_tokens'] == 3286
    assert forecast['file_tokens'] == costs
    assert forecast['tokens'] == 83286
    assert forecast['fits'] is True
    (root / 'requested.py').write_text('x\n' * (extra_tokens // 2))
    paths = [*inputs['plan']['files'], 'requested.py'] if extra_tokens else inputs['plan']['files']
    assert requested_fit(maker_request, paths) is expected


@pytest.mark.parametrize('overhead', [0, 25000, 45000])
@pytest.mark.parametrize('fix_round', [False, True])
def test_planner_fit_implies_maker_fit(maker_request, cache_tokenizer, monkeypatch, overhead, fix_round):
    from tl3_workers.forecast import Forecaster

    monkeypatch.setenv('TIKTOKEN_CACHE_DIR', cache_tokenizer)
    monkeypatch.chdir(maker_request['worktree'])
    inputs = maker_request['inputs']
    config = {'fit_tokens': 100000, 'context_tokens': 150000,
              'overhead_tokens': {'reserve': overhead}}
    total = Forecaster(config).measure(inputs['brief'], inputs['plan']['files'])['tokens']
    config['fit_tokens'] = total + 1
    inputs['forecast'] = config
    if fix_round:
        inputs.update(round=1, feedback=[{'kind': 'failed_check', 'report': 'Fix it. ' * 2000, 'goal_ids': ['greeting']}])
    assert Forecaster(config).measure(inputs['brief'], inputs['plan']['files'])['fits'] is True
    assert requested_fit(maker_request, inputs['plan']['files']) is True


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
    requests = [json.loads(message['data']['content']) for message in history
                if message['type'] == 'human' and not message['data']['content'].startswith('TL3 execution budget: ')]
    assert requests == [maker_request['inputs']], 'the fresh visit has exactly its current request inputs'
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


def test_bounded_fix_round_contract_accepts_resolved_records_and_finding_refs(maker_request):
    from tl3_workers.contracts import schema
    from jsonschema import Draft202012Validator
    inputs = maker_request['inputs']
    inputs.update(round=1, reviewed_sha='b' * 40, feedback=[
        {'kind': 'review_finding', 'id': 'review-1-0', 'reviewed_sha': 'a' * 40,
         'report': 'Greeting fails for an empty name.', 'goal_ids': ['greeting'], 'resolved_round': 1},
        {'kind': 'failed_check', 'report': 'Existing test failed.', 'goal_ids': [], 'resolved_round': 2},
        {'kind': 'review_finding', 'id': 'review-3-0', 'reviewed_sha': 'b' * 40,
         'report': 'Describe the signature in full.', 'goal_ids': ['greeting']},
    ])
    inputs['current_review_findings'] = [{'id': 'review-3-0', 'reviewed_sha': 'b' * 40}]
    assert Draft202012Validator(schema('maker-input')).is_valid(inputs), 'one-line records and finding refs'
    maker._check_inputs(inputs)
    reply = {**DONE, 'reproductions': [{'finding_id': 'review-3-0', 'reason': 'A documentation fix.'}]}
    maker._check_reply(inputs, reply)
    inputs['feedback'][0]['resolved_round'] = 0
    assert not Draft202012Validator(schema('maker-input')).is_valid(inputs), 'rounds start at 1'


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
    maker_request['model']['name'] = 'claude-opus-5-5'
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
    assert len(agent_calls(seen)) == len(turns)
    saved = json.loads(Path(maker_request['session_file']).read_text())
    assert any(message['data']['content'] == feedback for message in saved)


def test_no_change_done_recovers_through_protocol(tmp_path, maker_request):
    before = git(maker_request['worktree'], 'rev-parse', 'HEAD')
    maker_request['inputs']['attempt'] = 3
    maker_request['model']['name'] = 'claude-opus-5-5'
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
    assert len(agent_calls(observed)) == 51, 'a long maker session can finish its planned edits'
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
    assert len(agent_calls(observed)) == 65, 'maker allows 64 tool turns and one final answer'
    reply = events[-1]['reply']
    assert reply['kind'] == 'cant_meet'
    assert reply['budget_exhaustion']['dimension'] == 'tool_turns'
    assert reply['budget_exhaustion']['tool_turns'] == {'used': 64, 'allowed': 64}
    assert reply['budget_exhaustion']['unfinished_obligations']
    assert 'execution_budget_exhausted' in reply['report']


RECEIPT = {"action_key": "a" * 64, "status": "executed", "exit_code": 0,
           "receipt_sha256": "b" * 64, "log_sha256": "c" * 64}


def staged_tree(root):
    # The tree of the worktree's current files, as pb_tests stages them for PrismaBuild.
    with tempfile.TemporaryDirectory() as temporary:
        env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        env.update(GIT_INDEX_FILE=str(Path(temporary) / "index"), GIT_CONFIG_NOSYSTEM="1",
                   GIT_CONFIG_GLOBAL=os.devnull)
        subprocess.run(["git", "add", "--all"], cwd=root, env=env, check=True, capture_output=True)
        return subprocess.run(["git", "write-tree"], cwd=root, env=env, check=True,
                              capture_output=True, text=True).stdout.strip()


def receipts_factory(worktree):
    def run_tests(tests: list[str] | None = None, base: bool = False, repeat: int = 1,
                  parallel: int = 0, priority: int | None = None) -> dict:
        return {"status": "completed", "totals": {"passed": 1, "failed": 0, "error": 0, "skipped": 0},
                "tests": [], "note": "", "action_key": "a" * 64, "receipts": [RECEIPT],
                "tree": staged_tree(worktree.test_checkout),
                "repeat": repeat, "runs": [{"run": n + 1, "status": "completed",
                                            "totals": {"passed": 1, "failed": 0, "error": 0, "skipped": 0},
                                            "failed": []} for n in range(repeat)]}
    return run_tests


RUN_FIVE = AIMessage(**turn(calls=[{"name": "run_tests", "id": "test", "type": "tool_call",
                                    "args": {"tests": ["test_greeting.py::test_hello"], "repeat": 5}}]))


def test_done_reply_carries_the_run_tests_receipts(maker_request, monkeypatch):
    # #408: the evidence the maker cites lands in its reply for the reviewer,
    # for the tree that run tested, which is the committed head's tree (#432).
    fake = FakeMessagesListChatModel(responses=[
        AIMessage(**write_turn()),
        AIMessage(**write_turn("test_greeting.py", "def test_hello(): pass\n")),
        RUN_FIVE,
        AIMessage(**turn(DONE)),
    ])
    monkeypatch.setitem(TOOL_FACTORIES, "run_tests", receipts_factory)
    monkeypatch.setattr(runtime, "create_model", lambda request: fake)
    events = []
    runtime.run(maker_request, events.append)
    reply = events[-1]["reply"]
    assert reply["kind"] == "done"
    [evidence] = reply["test_evidence"]
    assert evidence["tests"] == ["test_greeting.py::test_hello"]
    assert evidence["repeat"] == 5 and len(evidence["runs"]) == 5
    assert evidence["receipts"] == [RECEIPT] and evidence["fresh"] is True
    assert evidence["tree"] == git(Path(maker_request["worktree"]), "rev-parse", f"{reply['head']}^{{tree}}")
    validate("maker-reply", reply)


def test_runs_before_a_later_edit_are_not_evidence_for_the_head(maker_request, monkeypatch):
    # #432: run, edit, done. The run tested a tree the head no longer has.
    root = Path(maker_request["worktree"])
    maker_request["inputs"]["check_base_sha"] = git(root, "rev-parse", "HEAD")
    fake = FakeMessagesListChatModel(responses=[
        AIMessage(**write_turn("test_greeting.py", "def test_hello(): pass\n")),
        RUN_FIVE,
        AIMessage(**write_turn()),
        AIMessage(**turn(DONE)),
    ])
    monkeypatch.setitem(TOOL_FACTORIES, "run_tests", receipts_factory)
    monkeypatch.setattr(runtime, "create_model", lambda request: fake)
    events = []
    runtime.run(maker_request, events.append)
    reply = events[-1]["reply"]
    assert reply["kind"] == "done"
    assert "test_evidence" not in reply
    assert reply["checking_handoff"]["head_tree"] == git(root, "rev-parse", reply["head"] + "^{tree}")
    assert reply["checking_handoff"]["status"] == "pending"


@pytest.mark.parametrize("special", [{"forcing": {"symptom": "Q55"}},
                                   {"helper_evidence": {"target": "test_greeting.py::helper"}}])
def test_ordinary_handoff_never_substitutes_for_forced_or_helper_proof(maker_request, special):
    inputs = maker_request["inputs"]
    inputs["check_base_sha"] = "a" * 40
    inputs["plan"]["checks"][0].update(special)
    assert maker._checking_handoff(inputs, "b" * 40, "c" * 40) is None


def test_test_evidence_is_bounded():
    # #432: entries, selectors and strings are capped, and the worker keeps the whole list small.
    from tl3_workers.tools import TEST_EVIDENCE_BYTES, Evidence

    entry = {"tests": ["test_greeting.py::test_hello"], "base": False, "repeat": 1, "parallel": 0,
             "status": "completed", "tree": "e" * 40, "fresh": True,
             "totals": {"passed": 1, "failed": 0, "error": 0, "skipped": 0}, "receipts": [RECEIPT]}
    done = {"kind": "done", "head": "f" * 40, "self_check": DONE["self_check"]}
    validate("maker-reply", {**done, "test_evidence": [entry]})
    for evidence in ([entry] * 11, [{**entry, "tests": ["t" * 2049]}], [{**entry, "tests": ["t"] * 257}],
                     [{**entry, "receipts": [RECEIPT] * 2}], [{**entry, "tree": "not a tree"}]):
        with pytest.raises(ValidationError):
            validate("maker-reply", {**done, "test_evidence": evidence})
    recorder = Evidence()
    for index in range(10):
        tests = tuple(f"test_{index}_{n}_" + "x" * 1900 for n in range(256))
        recorder.test_result({"status": "completed", "totals": entry["totals"], "receipts": [RECEIPT],
                              "tree": "e" * 40}, (False, tests), {"repeat": 1, "parallel": 0})
    kept = recorder.test_evidence()
    assert kept and len(json.dumps(kept).encode()) <= TEST_EVIDENCE_BYTES
    assert kept[-1]["tests"][0].startswith("test_9_"), "the newest run is kept"
    assert kept[-1]["tests_omitted"] == 256 - len(kept[-1]["tests"]) > 0
    validate("maker-reply", {**done, "test_evidence": kept})


def test_maker_input_carries_a_forced_check(maker_request):
    # #393: the maker sees the forcing its deflake must survive.
    check = maker_request['inputs']['plan']['checks'][0]
    check['forcing'] = {"symptom": "A real, unrelated process holds PID 4242.", "environment": {},
                        "patches": [{"target": "sampler.process_in_scope", "raises": "ValueError"}]}
    validate('maker-input', maker_request['inputs'])
    check['forcing']['code'] = 'print(1)'
    with pytest.raises(ValidationError):
        validate('maker-input', maker_request['inputs'])


def test_maker_prompt_explains_forced_checks():
    prompt = " ".join((Path(__file__).parents[1] / "src/tl3_workers/prompts/maker.md").read_text().split())
    assert "forcing" in prompt
    assert "both with and without the forcing" in prompt


@pytest.fixture
def request_123_handover(maker_request):
    """Sanitized fail-fast shape: test repair, Q55 proof, and mixed scope/PR goal."""
    request = maker_request
    request["inputs"]["goals"] = [
        {"id": "repair", "description": "Make the deep-path test deterministic.", "verification": "test"},
        {"id": "scope", "description": "Change only the declared ordinary test file.", "verification": "review"},
        {"id": "forced-proof", "description": "Independently prove forced base red and head green.",
         "verification": "test", "stage": "checking"},
        {"id": "publication", "description": "Publish a stable branch and non-draft PR with proof.",
         "verification": "review", "stage": "publishing",
         "publication": {"non_draft": True, "body_lines": ["Change type: test"], "proof": True}},
    ]
    request["inputs"]["plan"]["checks"] = [{
        "name": "deep-path", "kind": "demonstrating", "tests": ["test_greeting.py::test_hello"],
        "goal_ids": ["repair", "scope", "forced-proof"],
        "forcing": {"symptom": "The old test depends on residency.", "environment": {},
                    "patches": [{"target": "greeting.RESIDENCY", "value": False}]},
    }]
    reply = {"kind": "done", "self_check": [
        {"goal_id": "repair", "status": "met", "evidence": "Normal head run passed; plain base is unforced."},
        {"goal_id": "scope", "status": "met", "evidence": "Diff stays within the declared footprint."},
        {"goal_id": "forced-proof", "status": "pending", "why": "Controller Checking owns the Q55 verdict."},
        {"goal_id": "publication", "status": "pending", "why": "Publishing owns Git and PR creation."},
    ]}
    return request, reply


def test_request_123_honest_handover_with_downstream_pending(request_123_handover):
    request, reply = request_123_handover
    maker._check_inputs(request["inputs"])
    maker._check_reply(request["inputs"], reply)


def test_request_123_pending_handover_commits_through_real_worker(tmp_path, request_123_handover):
    request, reply = request_123_handover
    del request["inputs"]["plan"]["checks"][0]["forcing"]
    result = start_worker(tmp_path, request, [write_turn("test_greeting.py", "def test_hello(): assert True\n"), turn(reply)])
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]["reply"]["kind"] == "done"
    assert result.events[-1]["reply"]["self_check"] == reply["self_check"]


@pytest.mark.parametrize("goal_id", ["repair", "scope"])
def test_request_123_maker_cannot_defer_implementation_or_scope(request_123_handover, goal_id):
    request, reply = request_123_handover
    for check in reply["self_check"]:
        if check["goal_id"] == goal_id:
            check.clear()
            check.update(goal_id=goal_id, status="pending", why="Calling this downstream does not change ownership.")
    with pytest.raises(ValueError, match="maker"):
        maker._check_reply(request["inputs"], reply)


def test_request_123_maker_cannot_claim_future_proof(request_123_handover):
    request, reply = request_123_handover
    reply["self_check"][-1] = {"goal_id": "publication", "status": "met", "evidence": "Imaginary PR."}
    with pytest.raises(ValueError, match="pending"):
        maker._check_reply(request["inputs"], reply)
