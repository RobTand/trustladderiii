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
def test_subject_observer_proof_keeps_its_approved_semantics(role):
    prompt = prompt_text(role)
    assert "RED runs head observers against base subjects" in prompt
    assert "node_ids name the observers" in prompt
    assert "Every other base definition stays on base" in prompt
    assert "red_message (the exact call assertion message required on base)" in prompt
    assert (
        "Planned demonstrating runs with named head observers use base_semantics "
        "base_subject_head_observer and carry a proof receipt."
    ) in prompt
    assert "Outcome checks and unplanned base runs use untouched_base." in prompt
