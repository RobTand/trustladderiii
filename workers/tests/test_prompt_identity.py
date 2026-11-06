"""Pin the Python prompt contract for the Rust migration (#819, #730)."""
import json

import pytest

import prompt_identity_vectors as vectors
from tl3_workers.roles import ROLES


@pytest.mark.parametrize("name", ROLES)
def test_prompt_identity(name):
    expected = json.loads(vectors.VECTORS.read_text(encoding="utf-8"))
    assert vectors.role_vectors(ROLES[name]) == expected["roles"][name]


def test_vector_check():
    assert vectors.main(["--check"]) == 0


def test_maker_model_schema_excludes_controller_head():
    proposal = json.loads(vectors.schema_text(ROLES["maker"]))
    done = next(variant for variant in proposal["oneOf"]
                if variant["properties"]["kind"]["const"] == "done")
    assert "head" not in done["required"]
    assert "head" not in done["properties"]


def test_postmortem_model_schema_is_judge_result():
    contract = json.loads(vectors.schema_text(ROLES["postmortem"]))
    assert contract["$ref"] == "#/$defs/postmortem"
    assert "oneOf" not in contract


def test_reviewer_rendered_prompt_accepts_all_approved_red_classes():
    """tessera #1455: review must accept the state table's full demonstrating RED set."""
    from tl3_workers import runtime

    prompt = runtime.role_prompt(ROLES["reviewer"])
    proof = " ".join(prompt.split("## Subject/observer proof\n", 1)[1]
                     .split("\n## Generation", 1)[0].split())
    assert "red_message applies only to assertion RED" in proof, proof
    for instruction in (
        "an assertion failure",
        "a missing name that the leaf adds",
        "the explicitly declared symptom",
        "exact exception class",
        "exact test node",
        "call or setup phase",
        "declares these before the run",
        "original test's own code or fixture",
        "without red_message when the proof receipt shows the failure",
        "unrelated setup error, import failure, infrastructure failure, missing report, or outer hang never counts",
    ):
        assert instruction in proof, instruction
