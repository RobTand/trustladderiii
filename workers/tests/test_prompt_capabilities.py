from importlib.resources import files

import pytest


def prompt_text(role):
    return " ".join(
        files("tl3_workers").joinpath("prompts", f"{role}.md").read_text().split()
    )


# #648 condensed ingest and audit; the planner keeps its wording until it is condensed.
CAPABILITIES = {
    "planner": ("the maker has TL3 worktree file tools, run_tests through PrismaBuild and read-only recall",
                "the reviewer has read-only worktree tools, the supplied diff and check verdicts",
                "Do not require another repo, a fetched page or a shell"),
    "ingest": ("The maker has worktree file tools, run_tests through PrismaBuild and read-only recall",
               "The reviewer has read-only worktree tools, the diff and the check verdicts",
               "Never require another repository, a fetched page or a shell"),
    "audit": ("The maker has worktree file tools, run_tests through PrismaBuild and read-only recall",
              "The reviewer has read-only worktree tools, the diff and the check verdicts",
              "The brief must not require another repository, a fetched page or a shell"),
}
OUTSIDE_FACTS = {
    "planner": ("Preserve outside facts as the request gives them, name their source and mark them unverified",
                "goals that tests or review can judge inside this repo without outside verification"),
    "ingest": ("Keep each outside fact as the request states it, name its source and mark it unverified",
               "Write goals that tests or review can judge inside this repository"),
    "audit": ("Outside facts must keep their source and be marked unverified",
              "Missing outside confirmation alone is not an objection"),
}


@pytest.mark.parametrize("role", sorted(CAPABILITIES))
def test_briefs_and_plans_respect_downstream_capabilities(role):
    prompt = prompt_text(role)
    for statement in CAPABILITIES[role]:
        assert statement in prompt


@pytest.mark.parametrize("role", sorted(OUTSIDE_FACTS))
def test_outside_facts_stay_attributed_and_unverified(role):
    prompt = prompt_text(role)
    for statement in OUTSIDE_FACTS[role]:
        assert statement in prompt


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
        "Judge review-verified goals from the diff, the check verdicts "
        "and the head worktree" in prompt
    )
    assert (
        "Treat attributed outside facts as unverified. "
        "Do not require outside verification" in prompt
    )


@pytest.mark.parametrize("role", ["ingest", "audit"])
def test_narrowed_requests_keep_the_sections_the_answer_refers_to(role):
    # #449: Rob's answer is the scope; the original issue is context it may refer to.
    prompt = prompt_text(role)
    assert "Rob's answer (the new scope; it wins where it conflicts with the original issue)" in prompt
    assert "Original issue, narrowed by Rob's answer above" in prompt
    assert "Sections of the original issue that the answer refers to" in prompt
    assert "Question Rob answered" in prompt


def test_ingest_carries_referred_sections_into_the_brief():
    prompt = prompt_text("ingest")
    assert (
        "carry into the brief every fact from those sections that a kept goal depends on" in prompt
    )
    assert "drop only what the answer excludes" in prompt


def test_audit_objects_to_a_brief_missing_a_referred_section():
    prompt = prompt_text("audit")
    assert (
        "For each listed section, check that the brief carries what the kept goals depend on" in prompt
    )
    assert "or object" in prompt


def test_ingest_lists_only_the_goals_the_answer_keeps():
    # #456 review: the narrowing must not regrow the request Rob cut down.
    prompt = prompt_text("ingest")
    assert "every goal the answer keeps" in prompt
    assert "List every requested goal" not in prompt


def test_audit_objects_to_goals_the_answer_excludes():
    prompt = prompt_text("audit")
    assert (
        'Return "disagree" if the brief carries goals or acceptance tests that '
        "Rob's answer excludes or does not ask for" in prompt
    )


@pytest.mark.parametrize('role', ['ingest', 'audit', 'planner'])
def test_helper_location_alone_does_not_mandate_observer_evidence(role):
    prompt = prompt_text(role)
    assert 'The subject/observer rule' in prompt
    assert 'ordinary failing test' in prompt
    assert 'For a fix to a helper inside configured test files, require method' not in prompt


@pytest.mark.parametrize('role', ['maker', 'reviewer'])
def test_base_receipts_describe_planned_subject_observer_and_untouched_modes(role):
    prompt = prompt_text(role)
    assert 'base_subject_head_observer' in prompt
    assert 'untouched_base' in prompt
    assert 'proof receipt' in prompt
    assert 'Planned demonstrating runs with named head observers use base_semantics base_subject_head_observer' in prompt
    assert 'Outcome checks and unplanned base runs use untouched_base' in prompt
    assert 'Outcome checks include forcing with inferred subjects, declared-symptom checks and regression checks' in prompt
