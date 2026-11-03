"""Prompt rules from the #648 condensation review (#674)."""

from importlib.resources import files

import pytest

# The planner is condensed in PR 3, with #662; it gets its line there.
ROLES = ["maker", "ingest", "audit", "reviewer", "writer", "classifier", "postmortem"]


def prompt_text(role):
    return " ".join(files("tl3_workers").joinpath("prompts", role + ".md").read_text().split())


@pytest.mark.parametrize("role", ROLES)
def test_each_role_writes_its_own_text_in_asd_ste100(role):
    # Rob: agents are to be instructed to write in ASD-STE100.
    assert "Write your own text in ASD-STE100" in prompt_text(role)


@pytest.mark.parametrize("role", ["maker", "reviewer"])
def test_helper_evidence_keeps_its_semantic_checks_while_helper_mode_is_live(role):
    # Structural preservation receipts cannot show that a setup factory exercised the
    # failed execution: an empty queue lets the target raise without it.
    prompt = prompt_text(role)
    assert "A queue factory must claim the required row" in prompt
    assert "an empty queue alone cannot prove the failed execution" in prompt
    assert "the actual helper call and observer assertions" in prompt
