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
