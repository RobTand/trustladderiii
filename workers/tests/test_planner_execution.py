"""Planner execution budget and declared exploration contracts."""
# ruff: noqa: F811

import copy
from pathlib import Path

import pytest
from jsonschema import ValidationError

from tl3_workers.contracts import validate
from planner_fixtures import (
    LEAF,
    cache_tokenizer,  # noqa: F401
    execution_split,
    planner_request,  # noqa: F401
    tool_capable_fake,  # noqa: F401
)


def test_token_fitting_leaf_is_refused_when_execution_demand_exceeds_usable_budget(planner_request, monkeypatch):
    from tl3_workers.roles import planner

    monkeypatch.chdir(planner_request["worktree"])
    proposal = copy.deepcopy(LEAF)
    proposal["files"].extend(f"source_{i}.py" for i in range(4))
    existing = Path("tests/test_existing.py")
    existing.write_text(existing.read_text() + "\n".join(f"def test_extra_{i}(): assert True" for i in range(4)))
    proposal["checks"].extend({"kind": "regression", "node_ids": [f"tests/test_existing.py::test_extra_{i}"],
                               "goal_ids": [0, 1]} for i in range(4))
    planner.check_reply(planner_request["inputs"], proposal)
    assert proposal["forecast"]["fits"] is True, "the token forecast must stay truthful"
    assert any("execution budget" in item.lower() for item in proposal["violations"]), proposal
    execution = proposal["execution"]
    assert execution["estimated_tool_turns"] > execution["usable_tool_turns"] == 56
    assert execution["allowed_tool_turns"] == 64 and execution["finalization_turns"] == 8


def test_seven_file_public_output_adoption_requires_a_split_before_maker_work(planner_request, monkeypatch):
    from tl3_workers.roles import planner

    # PB #2717: seven token-fitting files consumed 35 edits, 20 reads, seven
    # searches and two test calls before completion. Five operations/file misses it.
    monkeypatch.chdir(planner_request["worktree"])
    proposal = copy.deepcopy(LEAF)
    proposal["files"].extend(f"public_output_{i}.py" for i in range(5))
    planner.check_reply(planner_request["inputs"], proposal)
    assert proposal["forecast"]["fits"], "execution pressure must not falsify token fit"
    assert any("execution budget" in item.lower() for item in proposal["violations"]), proposal
    assert proposal["execution"]["estimated_tool_turns"] > 56
    split = {"kind": "split", "reason": "execution_budget", "estimated_exploration_turns": 0, "pieces": [
        {"brief": "Adopt the public output and prove behavior, including metadata.",
         "goal_ids": [0], "files": proposal["files"][:4], "estimated_exploration_turns": 0},
        {"brief": "Finish adoption, preserve the interface and run final regressions.",
         "goal_ids": [0, 1], "files": proposal["files"][4:], "estimated_exploration_turns": 0}]}
    planner.check_reply(planner_request["inputs"], split)
    assert split["violations"] == [], split
    assert {path for piece in split["pieces"] for path in piece["files"]} == set(proposal["files"])
    assert {goal for piece in split["pieces"] for goal in piece["goal_ids"]} == {0, 1}
    assert all(piece["fits"] for piece in split["execution"]["pieces"])
    assert split["execution"]["aggregate_tool_turns"] == sum(
        piece["estimated_tool_turns"] for piece in split["execution"]["pieces"])
    assert split["execution"]["aggregate_allowed_tool_turns"] == 128


def test_execution_fit_requires_declared_exploration_and_refuses_the_three_file_search_shape(planner_request, monkeypatch):
    from tl3_workers import runtime
    from tl3_workers.forecast import execution_forecast
    from tl3_workers.roles import planner

    monkeypatch.chdir(planner_request["worktree"])
    proposal = copy.deepcopy(LEAF)
    proposal["files"].append("implementation.py")
    proposal.pop("estimated_exploration_turns", None)
    with pytest.raises(ValidationError, match='estimated_exploration_turns'):
        runtime._checked(planner.ROLE, planner_request['inputs'], proposal)
    assert execution_forecast(proposal['files'], len(proposal['checks']))['fits'] is False, \
        'structure alone cannot establish execution fit'
    proposal = copy.deepcopy(LEAF)
    proposal["files"].append("implementation.py")
    # PB #2718 spent 21 searches, 21 reads and one glob before implementation.
    proposal["estimated_exploration_turns"] = 43
    planner.check_reply(planner_request["inputs"], proposal)
    assert proposal["forecast"]["fits"] is True
    assert proposal["execution"]["components"]["exploration_operations"] == 43
    assert proposal["execution"]["fits"] is False
    assert any("execution budget" in item.lower() for item in proposal["violations"])


def test_execution_split_uses_existing_path_and_retains_aggregate_estimates(planner_request, monkeypatch):
    from tl3_workers.roles import planner

    monkeypatch.chdir(planner_request["worktree"])
    # Shared source footprints need not shrink in tokens when check work is split.
    planner_request["inputs"]["ingest"]["brief"] = "Preserve the greeting API and all required metadata and checks. " * 100
    proposal = execution_split()
    planner.check_reply(planner_request["inputs"], proposal)
    assert proposal["forecast"]["fits"] is True
    assert proposal["violations"] == [], proposal
    assert proposal.get("execution", {}).get("aggregate_tool_turns") == 50, proposal
    assert proposal["execution"]["aggregate_allowed_tool_turns"] == 128
    assert len(proposal["execution"]["pieces"]) == 2
    validate("planner-reply", proposal)


@pytest.mark.parametrize("damage", ["oversized_child", "too_many", "fitting_parent"])
def test_execution_split_is_bounded_before_work(planner_request, monkeypatch, damage):
    from tl3_workers.roles import planner

    monkeypatch.chdir(planner_request["worktree"])
    planner_request["inputs"]["ingest"]["brief"] *= 100
    proposal = execution_split()
    if damage == "oversized_child":
        proposal["pieces"][0]["estimated_tool_turns"] = 57
    elif damage == "too_many":
        proposal["pieces"] *= 5
    else:
        proposal["estimated_tool_turns"] = 20
    planner.check_reply(planner_request["inputs"], proposal)
    assert any("execution" in item.lower() for item in proposal["violations"]), proposal


GENERATED_BUILDS = ["BUILD.bazel", "src/prismabuild/BUILD.bazel", "tools/BUILD.bazel", "tests/BUILD.bazel"]
REQUEST236_TEST = "tests/test_a_kill_between_a_movers_two_documents_blocks_nothing.py"


@pytest.fixture
def request236(planner_request, monkeypatch):
    from tl3_workers.forcing_rehearsal import ForcingRehearsals, identity
    from tl3_harness import Forcing
    from tl3_workers.tools import Worktree

    root = Path(planner_request["worktree"])
    monkeypatch.chdir(root)
    Path(REQUEST236_TEST).write_text(
        "def test_first_document(): assert True\n"
        "def test_second_document(): assert True\n"
        "def test_regression(): assert True\n")
    Path("pyproject.toml").write_text('[tool.tl3.harness]\ntest-files = ["tests/**/*.py"]\n')
    for name in GENERATED_BUILDS:
        Path(name).parent.mkdir(parents=True, exist_ok=True)
        Path(name).write_text("# generated by tools/bazel/build_files.py\n")
    inputs = planner_request["inputs"]
    inputs["base_sha"] = "071094caa54922143398eb0397acb61bf3fed9f9"
    inputs["forecast"].update(generated_files=GENERATED_BUILDS.copy(),
                                generator="python3 tools/bazel/build_files.py")
    inputs["ingest"]["goals"] = [
        {"description": "Prove the first document remains safe.", "verification": "test"},
        {"description": "Prove the second document remains safe.", "verification": "test"}]
    proposal = dict(copy.deepcopy(LEAF), files=[REQUEST236_TEST], estimated_tool_turns=34,
                    estimated_exploration_turns=8, checks=[])
    # Recorded rehearsal identities exercise the real Q55 matcher without running served tests.
    with Worktree(root) as tree:
        ledger = ForcingRehearsals(tree, inputs)
        for index, name in enumerate(("test_first_document", "test_second_document")):
            forcing = {"symptom": f"The mover stops at document {index + 1}.", "environment": {},
                       "patches": [{"target": "mover.document", "value": index + 1}]}
            check = {"kind": "demonstrating", "goal_ids": [index],
                     "node_ids": [f"{REQUEST236_TEST}::{name}"], "forcing": forcing}
            proposal["checks"].append(check)
            ledger.receipts.append(identity(inputs["base_sha"], check["node_ids"], Forcing.parse(forcing)))
        proposal["checks"].append({"kind": "regression", "goal_ids": [0, 1],
                                   "node_ids": [f"{REQUEST236_TEST}::test_regression"]})
        yield inputs, proposal, ledger


def test_request236_generated_builds_fit_with_inspection_and_all_source_checks(request236):
    from tl3_workers.roles.planner import check_reply
    from tl3_workers.tools import Worktree

    inputs, proposal, ledger = request236
    checks = copy.deepcopy(proposal["checks"])
    check_reply(inputs, proposal, rehearsals=ledger)
    execution = proposal["execution"]
    assert execution["fits"], execution
    assert proposal["violations"] == [], proposal
    assert execution["estimated_tool_turns"] == 36
    assert execution["components"] == {
        "exploration_operations": 8, "file_operations": 9, "check_operations": 12,
        "scope_operations": 2, "generation_operations": 1, "generated_inspection_operations": 4}
    assert (execution["allowed_tool_turns"], execution["usable_tool_turns"], execution["finalization_turns"]) == (64, 56, 8)
    assert set(proposal["files"]) == {REQUEST236_TEST, *GENERATED_BUILDS}
    assert set(proposal["forecast"]["file_tokens"]) == set(proposal["files"])
    assert proposal["checks"] == checks
    validate("planner-reply", proposal)
    with Worktree(".", generated_files=inputs["forecast"]["generated_files"],
                  generator=inputs["forecast"]["generator"]) as tree:
        assert tree.generator == "python3 tools/bazel/build_files.py"
        for name in GENERATED_BUILDS:
            with pytest.raises(ValueError, match="Cannot hand-edit generated outputs"):
                tree._check_write(name)


@pytest.mark.parametrize("declared", [0, 80])
def test_leaf_budget_violation_names_every_component_and_value(request236, declared):
    from tl3_workers.roles.planner import check_reply

    inputs, proposal, ledger = request236
    inputs["ingest"]["brief"] = "UNTRUSTED_REQUEST_TEXT"
    proposal["estimated_exploration_turns"] = 31
    proposal["estimated_tool_turns"] = declared
    check_reply(inputs, proposal, rehearsals=ledger)
    execution = proposal["execution"]
    assert execution["estimated_tool_turns"] == max(59, declared)
    violation, = [item for item in proposal["violations"] if item.startswith("The leaf exceeds the execution budget:")]
    assert f"estimated {execution['estimated_tool_turns']} tool turns, usable 56" in violation
    for component, value in execution["components"].items():
        assert f"{component}={value}" in violation, violation
    assert "UNTRUSTED_REQUEST_TEXT" not in violation


def test_piece_budget_violation_names_every_component_and_value(request236):
    from tl3_workers.roles.planner import check_reply

    inputs, _, _ = request236
    proposal = {"kind": "split", "reason": "execution_budget", "estimated_exploration_turns": 43,
                "pieces": [
                    {"brief": "UNTRUSTED_PIECE_TEXT", "goal_ids": [index], "files": [REQUEST236_TEST],
                     "estimated_exploration_turns": exploration} for index, exploration in enumerate((43, 44))]}
    check_reply(inputs, proposal)
    violations = [item for item in proposal["violations"] if item.startswith("Every execution-budget piece must fit")]
    assert len(violations) == 2
    for violation, execution in zip(violations, proposal["execution"]["pieces"]):
        assert not execution["fits"]
        assert f"estimated {execution['estimated_tool_turns']} tool turns, usable 56" in violation
        for component, value in execution["components"].items():
            assert f"{component}={value}" in violation, violation
        assert "UNTRUSTED_PIECE_TEXT" not in violation


def test_five_authored_files_keep_the_original_68_turn_estimate(request236):
    from tl3_workers.roles.planner import check_reply

    inputs, proposal, ledger = request236
    inputs["forecast"]["generated_files"] = []
    proposal["files"].extend(f"source_{index}.py" for index in range(4))
    check_reply(inputs, proposal, rehearsals=ledger)
    assert proposal["execution"]["estimated_tool_turns"] == 68
    assert proposal["execution"]["components"]["file_operations"] == 45
    assert proposal["execution"]["fits"] is False
    assert any("execution budget" in item for item in proposal["violations"])


def test_generator_free_forecast_is_unchanged_even_for_generated_looking_names(request236):
    from tl3_workers.roles.planner import check_reply

    inputs, proposal, ledger = request236
    inputs["forecast"].pop("generator")
    proposal["files"].extend(GENERATED_BUILDS)
    check_reply(inputs, proposal, rehearsals=ledger)
    assert proposal["execution"] == {
        "allowed_tool_turns": 64, "finalization_turns": 8, "usable_tool_turns": 56,
        "estimated_tool_turns": 67, "exploration_declared": True, "fits": False,
        "components": {"exploration_operations": 8, "file_operations": 45,
                       "check_operations": 12, "scope_operations": 2, "generation_operations": 0}}


def test_generated_scope_and_inspection_are_deduplicated_for_each_split_child(request236):
    from tl3_workers.roles.planner import check_reply

    inputs, _, _ = request236
    inputs["forecast"]["generated_files"].extend(["**/BUILD.bazel", "BUILD.bazel"])
    proposal = {"kind": "split", "reason": "execution_budget", "estimated_tool_turns": 65,
                "estimated_exploration_turns": 8, "pieces": [
                    {"brief": "Prove document safety.", "goal_ids": [index], "files": [REQUEST236_TEST],
                     "estimated_exploration_turns": 8} for index in range(2)]}
    check_reply(inputs, proposal)
    assert proposal["violations"] == [], proposal
    assert set(proposal["forecast"]["file_tokens"]) == {REQUEST236_TEST, *GENERATED_BUILDS}
    assert proposal["execution"]["components"]["file_operations"] == 9
    for piece, execution in zip(proposal["pieces"], proposal["execution"]["pieces"]):
        assert set(piece["files"]) == {REQUEST236_TEST, *GENERATED_BUILDS}
        assert execution["components"]["file_operations"] == 9
        assert execution["components"]["generated_inspection_operations"] == 4
        assert execution["components"]["generation_operations"] == 1
        assert execution["estimated_tool_turns"] == 28
    assert proposal["execution"]["aggregate_tool_turns"] == 56
    validate("planner-reply", proposal)


def test_unrecognized_build_and_header_keep_manual_edit_cost_with_generator(request236):
    from tl3_workers.roles.planner import check_reply

    inputs, proposal, ledger = request236
    Path("other").mkdir()
    Path("other/BUILD.bazel").write_text("# generated by an unconfigured tool\n")
    proposal["files"].append("other/BUILD.bazel")
    check_reply(inputs, proposal, rehearsals=ledger)
    assert proposal["execution"]["components"]["file_operations"] == 18
    assert proposal["execution"]["components"]["generated_inspection_operations"] == 4


@pytest.mark.parametrize("declared, expected, fits", [(50, 50, True), (57, 57, False), (0, 36, True)])
def test_generated_forecast_preserves_declared_work_and_structural_minimum(request236, declared, expected, fits):
    from tl3_workers.roles.planner import check_reply

    inputs, proposal, ledger = request236
    proposal["estimated_tool_turns"] = declared
    check_reply(inputs, proposal, rehearsals=ledger)
    assert proposal["execution"]["estimated_tool_turns"] == expected
    assert proposal["execution"]["fits"] is fits
    assert bool(proposal["violations"]) is not fits
