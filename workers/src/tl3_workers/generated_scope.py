"""Reserve configured outputs without granting the model permission to edit them."""

from pathlib import PurePosixPath
import re

from tl3_harness.generator import path_parts


def generator_owned_outputs(paths, forecast):
    """Only configured ownership grants the generator's execution allowance.

    A header or familiar basename alone cannot prove the configured command
    produces a file. Include new glob matches even before they exist on disk.
    """
    if not forecast.get("generator"):
        return set()
    return {path for path in paths
            if any(PurePosixPath(path).full_match(pattern)
                   for pattern in forecast.get("generated_files", ()))}


def configured_outputs(worktree, forecast, protected=()):
    if not forecast.get("generator"):
        return []
    patterns = forecast.get("generated_files", ())
    outputs = {pattern for pattern in patterns if not any(char in pattern for char in "*?[")}
    if any(any(char in pattern for char in "*?[") for pattern in patterns):
        with worktree._directory(()) as root:
            outputs.update(generator_owned_outputs(
                (name for name, directory in worktree._walk(root) if not directory), forecast))
    for name in outputs:
        path_parts(name)
    return sorted(name for name in outputs
                  if not any(PurePosixPath(name).full_match(pattern) for pattern in protected))


# The audit model still checks meaning beyond these concrete phrasings.
_OUTPUT = re.compile(r"\bgenerated[ -](?:[\w-]+\s+){0,3}(?:files?|outputs?)\b|\bBUILD\.bazel\b|"
                     r"\bconfigured\s+(?:re)?generation\b", re.I)
_MANUAL = re.compile(r"\b(?:hand[ -]edit\w*|manual(?:ly)?|by hand)\b", re.I)
_FREEZE = re.compile(
    r"\b(?:keep|leave|remain|stay|preserve)\b.*\b(?:unchanged|untouched|unmodified)\b|"
    r"\bmust\s+(?:be|remain|stay)\s+(?:unchanged|untouched|unmodified)\b|"
    r"\b(?:prohibited|forbidden|disallowed|not permitted|not allowed)\b", re.I)
_NEGATIVE = re.compile(r"\b(?:do not|don't|don’t|never|must not|no)\b", re.I)
_ACTION = re.compile(r"\s*(?:change|modify|edit|regenerate|touch|run|"
                     r"be\s+(?:changed|modified|edited|regenerated|touched))\b", re.I)
_DISCUSSION = re.compile(
    r"\b(?:no|not|without)\s+(?:(?:such|a|any|new|blanket)\s+)*(?:rule|constraint|requirement|prohibition|ban)\b|"
    r"\b(?:do not|don't|don’t|never)\s+(?:add|impose|invent|treat|keep|leave|preserve)\b|"
    r"\b(?:need not|not required|not forbidden|not prohibited|false that|is false|"
    r"does not mean|not saying|must not stay|must not remain)\b|"
    r"\b(?:rejected?|denied|disputed?|invented|purported|quoted?|discuss(?:ed|es)?|whether|example)\b", re.I)
_QUOTED = re.compile(r'"[^"\n]*"|“[^”\n]*”|(?<!\w)\'[^\'\n]+\'(?!\w)|‘[^’\n]*’|`[^`\n]+`')
_FENCED = re.compile(r"^[ \t]*(?:```|~~~).*?(?:^[ \t]*(?:```|~~~)[^\n]*(?:\n|$)|\Z)", re.M | re.S)
_CLAUSE = re.compile(r"[,;:]|\b(?:but|however|yet)\b|\band\b", re.I)


class GeneratedOutputProhibition(ValueError):
    """A fixed diagnostic accepted by the existing one-turn reply repair."""

    message = "Remove the generated-output prohibition: the source restricts hand edits only"

    def __init__(self):
        super().__init__(self.message)


def _imposed_freeze(clause):
    if _DISCUSSION.search(clause):
        return False
    if _OUTPUT.search(clause) and _FREEZE.search(clause) and not _MANUAL.search(clause):
        return True
    for directive in _NEGATIVE.finditer(clause):
        subject = clause[:directive.start()]
        # A prohibition can coordinate hand edits and regeneration. Manual-only
        # wording must not hide a separate prohibition on the latter.
        for action in re.split(r"\bor\b", clause[directive.end():], flags=re.I):
            if (_ACTION.match(action) and not _MANUAL.search(action)
                    and (_OUTPUT.search(subject) or _OUTPUT.search(action))):
                return True
    return False


def output_freezes(text):
    # Quoted prose is evidence being discussed. Keep quoted paths and nouns so
    # that `BUILD.bazel` still identifies the target of an unquoted prohibition.
    def mask(match):
        return re.sub(r"[^\n]", " ", match[0])

    asserted = _FENCED.sub(mask, text)
    asserted = re.sub(r"^[ \t]*>.*$", mask, asserted, flags=re.M)
    asserted = _QUOTED.sub(lambda match: mask(match) if (_FREEZE.search(match[0]) or _NEGATIVE.search(match[0]))
                           else match[0], asserted)
    # Retain the whole sentence for audit provenance; scope denials to clauses.
    freezes, offset = [], 0
    for index, part in enumerate(re.split(r"((?<=[.!?])\s+|\n)", asserted)):
        if index % 2 == 0 and any(_imposed_freeze(clause) for clause in _CLAUSE.split(part)):
            freezes.append(text[offset:offset + len(part)].strip())
        offset += len(part)
    return freezes


def generation_scope_conflicts(inputs, ingest):
    if not any(capability["tool"] == "run_generator" and capability["executor"] == "prismabuild_generator"
               for capability in inputs.get("tool_capabilities", ())):
        return []
    issue = inputs["issue"]
    source = [("issue.title", issue.get("title", "")), ("issue.body", issue.get("body", ""))]
    conflicts = [(field, quote)
                 for field, text in source for quote in output_freezes(text)]
    if ingest["kind"] == "brief" and not conflicts:
        claims = [ingest["brief"], *(goal["description"] for goal in ingest["goals"])]
        conflicts = [("ingest", quote)
                     for claim in claims for quote in output_freezes(claim)]
    return conflicts


def check_ingest_generation_scope(inputs, reply):
    if any(field == "ingest" for field, _ in generation_scope_conflicts(inputs, reply)):
        raise GeneratedOutputProhibition()


def audit_generation_scope(inputs, reply):
    conflicts = []
    for field, quote in generation_scope_conflicts(inputs, inputs.get("ingest")):
        if field == "ingest":
            conflicts.append(f'Invented generated-output prohibition in ingest: "{quote}". '
                             'The source issue restricts model edits, not trusted configured generation. '
                             'Remove this added constraint and preserve the source wording.')
        else:
            conflicts.append(f'Configured generation conflicts with {field}: "{quote}". '
                             'Resolve this explicit generated-output prohibition before planning; '
                             'manual-edit restrictions alone do not prohibit configured generation.')
    if conflicts:
        reply["verdict"] = "disagree"
        reply["objections"] = list(dict.fromkeys([*reply["objections"], *conflicts]))
