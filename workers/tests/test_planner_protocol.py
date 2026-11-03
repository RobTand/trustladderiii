"""Planner protocol contracts."""
# ruff: noqa: F811

import copy
from pathlib import Path

import pytest

from planner_fixtures import (
    CANT_MEET,
    LEAF,
    SPLIT,
    assert_rejected,
    assert_rule_report,
    cache_tokenizer,  # noqa: F401
    planner_request,  # noqa: F401
    run_reply,
    tool_capable_fake,  # noqa: F401
)


def test_out_of_room_rejects_leaf(tmp_path, planner_request, monkeypatch):
    from tl3_workers.roles.planner import check_reply

    planner_request["inputs"]["must_split"] = True
    monkeypatch.chdir(planner_request["worktree"])
    reply = copy.deepcopy(LEAF)
    check_reply(planner_request["inputs"], reply)
    assert "Must split after running out of room." in reply["violations"]


def test_out_of_room_leaf_is_rejected_through_protocol(tmp_path, planner_request):
    planner_request["inputs"]["must_split"] = True
    assert_rule_report(run_reply(tmp_path, planner_request, LEAF), "Must split")


@pytest.mark.parametrize("proposal", [SPLIT, CANT_MEET])
def test_out_of_room_can_split_or_report_obstacle(tmp_path, planner_request, proposal):
    planner_request["inputs"]["must_split"] = True
    planner_request["inputs"]["findings"] = ["The maker exhausted its context."]
    proposal = copy.deepcopy(proposal)
    if proposal["kind"] == "split":
        proposal["reason"] = "out_of_room"
    result = run_reply(tmp_path, planner_request, proposal)
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]["reply"]["kind"] == proposal["kind"]


def test_protected_path_is_rejected_through_protocol(tmp_path, planner_request):
    assert_rule_report(run_reply(tmp_path, planner_request, {**LEAF, "files": ["conftest.py"]}), "protected path")


@pytest.mark.parametrize("field", ["must_split", "findings", "protected_paths"])
def test_controller_must_supply_planning_inputs(tmp_path, planner_request, field):
    del planner_request["inputs"][field]
    assert_rejected(run_reply(tmp_path, planner_request, LEAF), "request")


@pytest.mark.parametrize("proposal", [LEAF, SPLIT])
@pytest.mark.parametrize("path,pattern", [
    (".github/workflows/test.yml", ".github/**"),
    (".mergify.yml", ".mergify.yml"),
    ("conftest.py", "**/conftest.py"),
    ("tests/nested/conftest.py", "**/conftest.py"),
    ("pytest.ini", "pytest.ini"),
    ("tox.ini", "tox.ini"),
    ("noxfile.py", "noxfile.py"),
    ("setup.cfg", "setup.cfg"),
    ("pyproject.toml", "pyproject.toml"),
    ("src/private.py", "src/*.py"),
])
def test_protected_paths_are_rejected(tmp_path, planner_request, monkeypatch, proposal, path, pattern):
    from tl3_workers.roles.planner import check_reply

    planner_request["inputs"]["protected_paths"] = [pattern]
    reply = copy.deepcopy(proposal)
    if reply["kind"] == "leaf":
        reply["files"] = [path]
    else:
        reply["pieces"][1]["files"] = [path]
    monkeypatch.chdir(planner_request["worktree"])
    check_reply(planner_request["inputs"], reply)
    assert any("protected path" in report for report in reply["violations"])


@pytest.mark.parametrize("change,reported", [
    ({"checks": []}, True),
    ({"checks": [LEAF["checks"][0]]}, True),
    ({"checks": [{**LEAF["checks"][0], "goal_ids": [0, 1, 2]}]}, True),
    ({"checks": [{**LEAF["checks"][0], "goal_ids": []}]}, True),
    ({"checks": [{**LEAF["checks"][0], "node_ids": []}]}, True),
    ({"checks": [{**LEAF["checks"][0], "kind": "guess"}]}, False),
    ({"checks": [{**LEAF["checks"][0], "node_ids": ["tests/test_greeting.py"]}]}, True),
    ({"files": []}, True),
    ({"files": ["greeting.py", "greeting.py"]}, False),
    ({"no_design_questions": False}, True),
    ({"lands_on_its_own": False}, True),
])
def test_leaf_rules_are_reported_and_malformed_shapes_fail(tmp_path, planner_request, change, reported):
    result = run_reply(tmp_path, planner_request, {**LEAF, **change})
    if reported:
        assert_rule_report(result)
    else:
        assert_rejected(result)


def test_invalid_node_id_is_reported_to_controller(tmp_path, planner_request):
    proposal = copy.deepcopy(LEAF)
    proposal["checks"][0]["node_ids"] = ["../test.py::test_escape"]
    result = run_reply(tmp_path, planner_request, proposal)
    assert_rule_report(result)
    assert result.events[-1]["reply"]["violations"] == [
        "Checks must name exact pytest node IDs. Replace `../test.py::test_escape` "
        "with the exact test definitions this check needs."]


@pytest.mark.parametrize("path", [
    "../outside.py", "/etc/passwd", "new/../../outside.py", "new/../outside.py",
    "new\\outside.py", ".git/config", "new/.git/config", "./greeting.py",
    "new//file.py", "greeting.py/", ".", "bad\x00path",
])
def test_declared_paths_are_confined(tmp_path, planner_request, path):
    result = run_reply(tmp_path, planner_request, {**LEAF, "files": [path]})
    # #321: a named violation, not a crash. Nothing outside the worktree is measured.
    assert_rule_report(result, f"Declared file can't be used: {path}: ")
    assert result.events[-1]["reply"]["forecast"]["file_tokens"][path] == 0


@pytest.mark.parametrize("proposal,reported", [
    ({**SPLIT, "pieces": SPLIT["pieces"][:1]}, True),
    ({**SPLIT, "reason": "convenience"}, False),
    ({**SPLIT, "pieces": [SPLIT["pieces"][0], SPLIT["pieces"][0]]}, True),
    ({**SPLIT, "pieces": [SPLIT["pieces"][0], {**SPLIT["pieces"][1], "goal_ids": [99]}]}, True),
    ({**SPLIT, "pieces": [SPLIT["pieces"][0], {**SPLIT["pieces"][1], "files": ["../escape"]}]}, True),
    ({**CANT_MEET, "report": ""}, False),
    ({**CANT_MEET, "files": ["../escape"]}, True),
])
def test_split_rules_are_reported_and_malformed_shapes_fail(tmp_path, planner_request, proposal, reported):
    result = run_reply(tmp_path, planner_request, proposal)
    if reported:
        assert_rule_report(result)
    else:
        assert_rejected(result)


def test_early_ingest_end_is_not_plannable(tmp_path, planner_request):
    planner_request["inputs"]["ingest"] = {
        "kind": "early_end", "outcome": "duplicate", "reason": "Another request covers it.",
    }
    assert_rejected(run_reply(tmp_path, planner_request, LEAF), "request")


def test_read_only_tools_through_protocol(tmp_path, planner_request):
    calls = [
        {"name": name, "args": args, "id": name, "type": "tool_call"}
        for name, args in [
            ("list", {}), ("read", {"path": "greeting.py"}),
            ("glob", {"pattern": "*.py"}), ("grep", {"pattern": "hello"}),
            ("write", {"path": "forbidden.py", "content": "no"}),
            ("edit", {"path": "greeting.py", "old": "hello", "new": "changed"}),
        ]
    ]
    result = run_reply(tmp_path, planner_request, copy.deepcopy(LEAF), calls)
    assert result.exit_code == 0, result.stderr
    assert result.events[:-1] == [
        {"event": "progress", "unit": "model_turn"},
        *[{"event": "progress", "unit": "tool_result"} for _ in calls],
        {"event": "progress", "unit": "model_turn"},
    ]
    assert not (tmp_path / "worktree/forbidden.py").exists()
    assert (tmp_path / "worktree/greeting.py").read_text() == "hello world\n"
    assert result.events[-1]["reply"]["forecast"]["file_tokens"]["greeting.py"] == 3


def test_model_cannot_forge_rule_evidence(tmp_path, planner_request):
    assert_rejected(run_reply(tmp_path, planner_request, {**LEAF, "violations": []}))


def test_split_requires_evidence_and_smaller_children(tmp_path, planner_request):
    reply = run_reply(tmp_path, planner_request, SPLIT)
    assert_rule_report(reply, "forecast fits")
    assert_rule_report(reply, "smaller forecast")


@pytest.mark.parametrize("pattern", ["tests/BUILD.bazel", "**/BUILD.bazel"])
def test_request_2709_plan_reserves_generated_outputs(planner_request, monkeypatch, pattern):
    from tl3_workers.roles.planner import check_reply
    monkeypatch.chdir(planner_request["worktree"])
    Path("tests/BUILD.bazel").write_text("# Generated by configured generator\n")
    config = planner_request["inputs"]["forecast"]
    config.update(generator="python3 generate.py", generated_files=[pattern], generator_available=True)
    reply = copy.deepcopy(LEAF)
    manual = list(reply["files"])
    check_reply(planner_request["inputs"], reply)
    assert not reply["violations"], reply
    assert reply["files"] == sorted([*manual, "tests/BUILD.bazel"])
    assert reply["forecast"]["file_tokens"]["tests/BUILD.bazel"] == 1024


def test_generated_footprint_never_reserves_protected_output(planner_request, monkeypatch):
    from tl3_workers.roles.planner import check_reply
    monkeypatch.chdir(planner_request["worktree"])
    Path("tests/BUILD.bazel").write_text("# Generated by configured generator\n")
    planner_request["inputs"]["forecast"].update(
        generator="python3 generate.py", generated_files=["**/BUILD.bazel"], generator_available=True)
    planner_request["inputs"]["protected_paths"].append("tests/BUILD.bazel")
    reply = copy.deepcopy(LEAF)
    check_reply(planner_request["inputs"], reply)
    assert "tests/BUILD.bazel" not in reply["files"]
