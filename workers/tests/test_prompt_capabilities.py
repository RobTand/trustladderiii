from importlib.resources import files

import pytest


def prompt_text(role):
    return " ".join(
        files("tl3_workers").joinpath("prompts", f"{role}.md").read_text().split()
    )


@pytest.mark.parametrize("role", ["ingest", "audit", "planner"])
def test_briefs_and_plans_respect_downstream_capabilities(role):
    prompt = prompt_text(role)
    assert (
        "the maker has TL3 worktree file tools, run_tests through PrismaBuild "
        "and read-only recall" in prompt
    )
    assert (
        "the reviewer has read-only worktree tools, the supplied diff "
        "and check verdicts" in prompt
    )
    assert "Do not require another repo, a fetched page or a shell" in prompt


@pytest.mark.parametrize("role", ["ingest", "audit", "planner"])
def test_outside_facts_stay_attributed_and_unverified(role):
    prompt = prompt_text(role)
    assert (
        "Preserve outside facts as the request gives them, name their source "
        "and mark them unverified" in prompt
    )
    assert (
        "goals that tests or review can judge inside this repo "
        "without outside verification" in prompt
    )


def test_planner_review_goals_name_available_evidence():
    prompt = prompt_text("planner")
    assert (
        "For each review-verified goal, state what the reviewer must judge" in prompt
    )
    assert (
        "using the supplied diff, check verdicts and readable head worktree" in prompt
    )


def test_reviewer_judges_review_goals_without_outside_verification():
    prompt = prompt_text("reviewer")
    assert (
        "Judge review-verified goals using the supplied diff, check verdicts "
        "and readable head worktree" in prompt
    )
    assert (
        "Treat attributed outside facts as unverified; "
        "do not require outside verification" in prompt
    )
