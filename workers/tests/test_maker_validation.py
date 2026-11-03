# ruff: noqa: F811

from dataclasses import asdict
from importlib.resources import files
import json
from pathlib import Path
from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage, ToolMessage
import pytest
from tl3_workers import runtime
from tl3_workers.contracts import validate
from tl3_workers.roles import ROLES
from tl3_workers.roles import maker
from tl3_workers.tools import TOOL_FACTORIES
from worker_process import run_real_entrypoint, start_worker
from message_invariants import DONE as DONE
from message_invariants import RETRO_TURN as RETRO_TURN
from message_invariants import ToolModel as ToolModel
from message_invariants import git as git
from message_invariants import maker_request as maker_request
from message_invariants import tool_capable_process_model as tool_capable_process_model
from message_invariants import turn as turn
from message_invariants import write_turn as write_turn


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
    assert "label a failure pre-existing only for tests that regression checks name" in prompt
    assert "fails-on-base means that the test also failed or errored on base, but no regression check names it." in prompt
    assert "It excuses no demonstrating, reproducing or behavior-preserving check" in prompt
    assert "do not edit unrelated code, pins or tests to make a test pass" in prompt
    assert "Name such a test and both runs in your self-check" in prompt
    assert "so it is not a reason for cant_meet" in prompt
    assert "For a goal that review verifies, describe the change and how it meets the goal" in prompt
    assert "Mark evidence that your tools cannot reach as unverified. Review decides" in prompt
    assert "A failing check that your change caused is never unverified." in prompt


def test_prompt_tells_history_records_from_open_feedback():
    # #316: superseded findings arrive as one-line records with resolved_round.
    prompt = " ".join(files("tl3_workers").joinpath("prompts", "maker.md").read_text().split())
    assert "In a fix round, fix every open feedback item. An item with resolved_round is history, not a task." in prompt


def test_prompt_requires_reds_the_harness_accepts():
    # #340: a module-level import of a new name errors on the base, and a flaky
    # test can pass there; neither demonstrates the change.
    prompt = " ".join(files("tl3_workers").joinpath("prompts", "maker.md").read_text().split())
    assert "must fail by assertion on the base" in prompt
    assert "Import the names that your change adds inside the test body" in prompt
    assert "the base run fails as a test, not at collection" in prompt
    assert "Prefer new test files when the plan allows them" in prompt
    assert "force the bad timing with injected clocks, events or barriers" in prompt
    assert "It cannot prove RED for an ordinary demonstrating check" in prompt
    assert "Checking proves that RED after your reply" in prompt
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
    inputs["tests"].update(cpus=2, mem_gb=3, quiet_seconds=60)

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
        RETRO_TURN,
        {"event": "reply", "role": "maker", "reply": response},
    ]
    assert not (Path(maker_request["worktree"]) / "forbidden").exists()
