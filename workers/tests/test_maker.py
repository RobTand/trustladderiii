# ruff: noqa: F811

from pathlib import Path
import pytest
from tl3_workers.contracts import validate
from tl3_workers.roles import ROLES
from tl3_workers.tools import READ_TOOLS, WRITE_TOOLS
from worker_process import start_worker
from message_invariants import DONE as DONE
from message_invariants import RETRO_TURN as RETRO_TURN
from message_invariants import git as git
from message_invariants import maker_request as maker_request
from message_invariants import tool_capable_process_model as tool_capable_process_model
from message_invariants import turn as turn
from message_invariants import write_turn as write_turn


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
                    "mode": "base_subject_head_observer"}],
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
        *([] if reply["kind"] == "out_of_room" else [RETRO_TURN]),
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
        "model_turn", "tool_result", "model_turn", "tool_result", "model_turn", RETRO_TURN["unit"],
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
