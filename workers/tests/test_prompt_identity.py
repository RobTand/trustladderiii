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
