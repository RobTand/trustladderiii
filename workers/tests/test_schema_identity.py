"""Whitespace normalization must preserve values and insertion order (#819)."""
from importlib.resources import files
import json
from pathlib import Path

import pytest


BASE = json.loads(Path(__file__).with_name("schema-base-pairs.json").read_text(encoding="utf-8"))
SCHEMAS = files("tl3_workers").joinpath("schemas")


def test_schema_inventory():
    assert sorted(path.name for path in SCHEMAS.iterdir() if path.name.endswith(".json")) == sorted(BASE["schemas"])


@pytest.mark.parametrize("name", BASE["schemas"])
def test_schema_values_and_key_order(name):
    pairs = json.loads(SCHEMAS.joinpath(name).read_text(encoding="utf-8"), object_pairs_hook=list)
    # JSON converts Python tuples returned by object_pairs_hook into arrays.
    assert json.loads(json.dumps(pairs)) == BASE["schemas"][name]


@pytest.mark.parametrize("name", BASE["schemas"])
def test_schema_canonical_form(name):
    text = SCHEMAS.joinpath(name).read_text(encoding="utf-8")
    assert text == json.dumps(json.loads(text), indent=2, ensure_ascii=False) + "\n"
