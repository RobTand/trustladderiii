"""Load the same JSON Schemas that the controller consumes."""

from functools import cache
from importlib.resources import files
import json

from jsonschema import Draft202012Validator
from referencing import Registry, Resource


SCHEMA_BASE = "https://trustladderiii.invalid/workers/protocol-1/"


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


def _fenced_json(text):
    """Bodies of closed ``` or ```json blocks; blocks in other languages are not candidates."""
    blocks, body, language = [], None, None
    for line in text.splitlines():
        marker = line.strip()
        if marker.startswith("```"):
            if body is None:
                body, language = [], marker[3:].strip().lower()
            else:
                if language in ("", "json"):
                    blocks.append("\n".join(body))
                body = None
        elif body is not None:
            body.append(line)
    return blocks


def read_reply(text):
    """Parse a model reply: strict JSON, else its one fenced block, else its one outer object.

    Claude models often wrap the object in prose or a fence (tl3-sandbox#229, #230).
    Whatever is extracted is parsed by read_json, so its checks still apply."""
    try:
        return read_json(text)
    except json.JSONDecodeError:
        pass
    fenced = _fenced_json(text)
    if len(fenced) > 1:
        raise ValueError("reply has more than one JSON object")
    if fenced:
        return read_json(fenced[0])
    objects, index, decoder = [], 0, json.JSONDecoder()
    while (index := text.find("{", index)) != -1:
        try:
            _, end = decoder.raw_decode(text, index)
        except json.JSONDecodeError:
            index += 1
            continue
        objects.append(text[index:end])
        index = end
    if len(objects) > 1:
        raise ValueError("reply has more than one JSON object")
    if not objects:
        raise ValueError("reply is not one JSON object")
    return read_json(objects[0])


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
