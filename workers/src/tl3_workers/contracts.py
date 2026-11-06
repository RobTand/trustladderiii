"""Load the same JSON Schemas that the controller consumes."""

from copy import deepcopy
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


def _refs(value):
    if isinstance(value, dict):
        for key, item in value.items():
            if key == "$ref" and isinstance(item, str):
                yield value
            else:
                yield from _refs(item)
    elif isinstance(value, list):
        for item in value:
            yield from _refs(item)


def model_facing(reply_schema):
    """The reply schema as the model sees it in its prompt (#648).

    The model cannot follow a $ref into another schema file, so each one is
    copied into $defs. A $defs entry that nothing references is dropped. Worker
    code still validates replies against the full schema and its registry."""
    result = deepcopy(reply_schema)
    defs = result.setdefault("$defs", {})
    pending = list(_refs(result))
    bundled = {}
    while pending:
        node = pending.pop()
        source, _, pointer = node["$ref"].partition("#")
        if not source or not pointer.startswith("/$defs/"):
            continue
        key = (source, pointer)
        if key not in bundled:
            name = pointer.removeprefix("/$defs/")
            if name in defs:
                name = source.removesuffix(".json").replace("-", "_") + "_" + name
            bundled[key] = name
            copied = deepcopy(schema(source.removesuffix(".json"))["$defs"][pointer.removeprefix("/$defs/")])
            for inner in _refs(copied):
                # The copied definition's own local references point into its source file.
                if inner["$ref"].startswith("#"):
                    inner["$ref"] = source + inner["$ref"]
            defs[name] = copied
            pending.extend(_refs(copied))
        node["$ref"] = "#/$defs/" + bundled[key]
    used, frontier = set(), [result[key] for key in result if key != "$defs"]
    while frontier:
        for node in _refs(frontier.pop()):
            name = node["$ref"].removeprefix("#/$defs/")
            if node["$ref"].startswith("#/$defs/") and name not in used and name in defs:
                used.add(name)
                frontier.append(defs[name])
    result["$defs"] = {name: value for name, value in defs.items() if name in used}
    if not result["$defs"]:
        del result["$defs"]
    return result


def _applicable_errors(error):
    """Select a discriminated branch; retain alternatives when no branch is known."""
    if error.validator not in ('oneOf', 'anyOf') or not error.context:
        yield error
        return
    variants = error.validator_value
    discriminators = set.intersection(*({key for key, item in variant.get('properties', {}).items()
                                        if 'const' in item or 'enum' in item}
                                       for variant in variants))
    matching = []
    for index, variant in enumerate(variants):
        constants = {key: item.get('enum', [item.get('const')])
                     for key, item in variant.get('properties', {}).items()
                     if key in discriminators}
        if constants and isinstance(error.instance, dict) and all(
                key in error.instance and error.instance[key] in values
                for key, values in constants.items()):
            matching.append(index)
    if len(matching) != 1:
        yield error
        return
    for child in error.context:
        if child.schema_path[0] == matching[0]:
            yield from _applicable_errors(child)


def schema_violations(errors, root_schema=None):
    """Translate every schema failure into facts, without jsonschema's reply echo."""
    from dataclasses import replace
    from tl3_workers.rules import Allowed, Edit, Name, Rule, Violation, at

    def check(facts):
        try:
            value = at(facts['reply'], facts['pointer'])
        except (KeyError, IndexError, TypeError):
            return () if facts.get('op') == 'remove' else (facts,)
        if facts.get('op') == 'remove':
            return ({**facts, 'bad_value': value},)
        return () if facts['allowed'].accepts(value) else ({**facts, 'bad_value': value},)

    def names(facts):
        return (Name('field', facts['pointer']),)

    row = Rule('schema.validation', check, names, lambda facts: facts['allowed'],
               lambda facts: Edit(Name('field', facts['pointer']), facts.get('op', 'replace'), facts['allowed']))
    result = []
    for parent in errors:
        for error in _applicable_errors(parent):
            pointer = ''.join('/' + str(part).replace('~', '~0').replace('/', '~1')
                              for part in error.absolute_path)
            if error.validator == 'additionalProperties' and error.validator_value is False:
                import re
                extra = set(error.instance) - error.schema.get('properties', {}).keys()
                extra = sorted(key for key in extra if not any(re.search(pattern, key)
                               for pattern in error.schema.get('patternProperties', {})))
                for key in extra:
                    result.append(Violation(replace(row, id='schema.additionalproperties'), {
                        'pointer': pointer + '/' + key.replace('~', '~0').replace('/', '~1'),
                        'allowed': Allowed('enum', []), 'op': 'remove', 'bad_value': error.instance[key]}))
                continue
            if error.validator in ('oneOf', 'anyOf', 'not', 'unevaluatedProperties'):
                context = error
                while context.parent is not None:
                    context = context.parent
                source = root_schema or context.schema
                constraint = {**error.schema, '$defs': source.get('$defs', {})}
                if '$id' in source:
                    constraint['$id'] = source['$id']
                allowed = Allowed('schema', constraint)
            else:
                allowed = Allowed(error.validator, error.validator_value)
            result.append(Violation(replace(row, id='schema.' + error.validator.lower()),
                                    {'pointer': pointer, 'allowed': allowed,
                                          'bad_value': error.instance}))
    return tuple(result)


def reply_errors(name, value):
    return tuple(Draft202012Validator(schema(name), registry=registry()).iter_errors(value))
