"""Whitespace normalization must preserve values and insertion order (#819)."""
import hashlib
from importlib.resources import files
import json
from pathlib import Path

import pytest


BASE = json.loads(Path(__file__).with_name("schema-base-pairs.json").read_text(encoding="utf-8"))
CURRENT = json.loads(Path(__file__).with_name("schema-current-pairs.json").read_text(encoding="utf-8"))
CLOSURE_PATHS = {
    "planner-input.json": ("properties",),
    "postmortem-input.json": ("properties", "case", "properties"),
}
SCHEMAS = files("tl3_workers").joinpath("schemas")


def test_schema_inventory():
    assert hashlib.sha256(Path(__file__).with_name("schema-base-pairs.json").read_bytes()).hexdigest() == (
        "601e53cf2b07578499a15b1f026ae9e374add04d88fa1098c02c6e7c715e9520"
    )
    assert set(CURRENT["schemas"]) == set(CLOSURE_PATHS) | {"maker-input.json", "maker-reply.json", "check-report.json",
                                                                      "audit-input.json", "ingest-input.json"}
    # #832 and #826 add worker/operator contracts outside every model prompt.
    expected = set(BASE["schemas"]) | {"plan-check-provenance.json", "execution-failure.json",
                                       "worker-execution.json"}
    assert {path.name for path in SCHEMAS.iterdir() if path.name.endswith(".json")} == expected


@pytest.mark.parametrize("name", BASE["schemas"])
def test_schema_values_and_key_order(name):
    pairs = json.loads(SCHEMAS.joinpath(name).read_text(encoding="utf-8"), object_pairs_hook=list)
    # JSON converts Python tuples returned by object_pairs_hook into arrays.
    expected = CURRENT["schemas"].get(name, BASE["schemas"][name])
    assert json.loads(json.dumps(pairs)) == expected


@pytest.mark.parametrize("name", BASE["schemas"])
def test_schema_canonical_form(name):
    text = SCHEMAS.joinpath(name).read_text(encoding="utf-8")
    assert text == json.dumps(json.loads(text), indent=2, ensure_ascii=False) + "\n"


@pytest.mark.parametrize("name", CLOSURE_PATHS)
def test_t839_current_schema_preserves_frozen_values_and_order(name):
    # #839 adds only closure evidence; every frozen field and its order survives.
    pairs = json.loads(json.dumps(CURRENT["schemas"][name]))
    properties = pairs
    for key in CLOSURE_PATHS[name]:
        properties = dict(properties)[key]
    assert [key for key, _ in properties].count("prerequisite_closures") == 1
    properties[:] = [pair for pair in properties if pair[0] != "prerequisite_closures"]
    assert pairs == BASE["schemas"][name]
