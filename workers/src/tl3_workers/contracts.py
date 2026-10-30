"""Load the same JSON Schemas that the controller consumes."""

from functools import cache
from importlib.resources import files
import json

from jsonschema import Draft202012Validator
from referencing import Registry, Resource


SCHEMA_BASE = "https://trustladderiii.invalid/workers/protocol-1/"
ROLES = ("ingest", "audit")


def _object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON key")
        result[key] = value
    return result


def _invalid_constant(value):
    raise ValueError("non-finite JSON number")


def read_json(document):
    return json.loads(document, object_pairs_hook=_object, parse_constant=_invalid_constant)


@cache
def schema(name):
    return read_json(files("tl3_workers").joinpath("schemas", name + ".json").read_text())


@cache
def registry():
    return Registry().with_resources(
        (SCHEMA_BASE + name, Resource.from_contents(read_json(path.read_text())))
        for path in files("tl3_workers").joinpath("schemas").iterdir()
        if (name := path.name).endswith(".json")
    )


def validate(name, value):
    Draft202012Validator(schema(name), registry=registry()).validate(value)
