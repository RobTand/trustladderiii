from dataclasses import asdict
from importlib.resources import files
import json
import os
from pathlib import Path
import subprocess

from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
import pytest

from tl3_workers import runtime
from tl3_workers.contracts import validate
from tl3_workers.roles import ROLES
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
    assert ROLES["maker"].tools.keys() == READ_TOOLS | WRITE_TOOLS | {"run_tests"}


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


@pytest.mark.parametrize("turns", [[turn(DONE)], [write_turn(content="def greet(): return 'goodbye'\n"), turn(DONE)]])
def test_done_without_a_change_is_rejected(tmp_path, maker_request, turns):
    root = Path(maker_request["worktree"])
    before = git(root, "rev-parse", "HEAD")
    result = start_worker(tmp_path, maker_request, turns)
    assert result.exit_code == 1
    assert all(event["event"] != "reply" for event in result.events)
    assert git(root, "rev-parse", "HEAD") == before


def test_done_can_report_an_unmet_goal_with_a_reason(tmp_path, maker_request):
    reply = {"kind": "done", "self_check": [DONE["self_check"][0],
             {"goal_id": "interface", "status": "not_met", "why": "Signature check is still failing."}]}
    result = start_worker(tmp_path, maker_request, [write_turn(), turn(reply)])
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]["reply"]["self_check"] == reply["self_check"]


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


def test_missing_usage_fails_before_tools(tmp_path, maker_request):
    response = write_turn("must-not-exist")
    del response["usage_metadata"]
    result = start_worker(tmp_path, maker_request, [response, turn(DONE)])
    assert result.exit_code == 1
    assert not (Path(maker_request["worktree"]) / "must-not-exist").exists()
    assert all(event["event"] != "reply" for event in result.events)


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


def test_fix_round_inputs_and_test_settings_reach_their_destinations(tmp_path, maker_request, monkeypatch):
    observed = []
    settings = []
    inputs = maker_request["inputs"]
    inputs.update({"attempt": 2, "round": 1, "feedback": [
        {"kind": "failed_check", "report": "test_hello failed.", "goal_ids": ["greeting"]},
        {"kind": "review_finding", "report": "Keep greet's signature.", "goal_ids": ["interface"]},
    ]})
    inputs["tests"].update(cpus=2, mem_gb=3, quiet_seconds=60)

    def factory(worktree):
        settings.append(asdict(worktree.test_settings))

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
