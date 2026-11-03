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
