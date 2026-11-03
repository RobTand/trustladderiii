"""Reserve configured outputs without granting the model permission to edit them."""

from pathlib import PurePosixPath
import re

from tl3_harness.generator import path_parts


def configured_outputs(worktree, forecast, protected=()):
    if not forecast.get("generator"):
        return []
    patterns = forecast.get("generated_files", ())
    outputs = {pattern for pattern in patterns if not any(char in pattern for char in "*?[")}
    if any(any(char in pattern for char in "*?[") for pattern in patterns):
        with worktree._directory(()) as root:
            outputs.update(name for name, directory in worktree._walk(root)
                           if not directory and any(PurePosixPath(name).full_match(pattern) for pattern in patterns))
    for name in outputs:
        path_parts(name)
    return sorted(name for name in outputs
                  if not any(PurePosixPath(name).full_match(pattern) for pattern in protected))


# Detect blanket output freezes, not restrictions on manual edits. The audit
# model still checks scope and source meaning beyond these concrete phrasings.
_OUTPUT = re.compile(r"\bgenerated[ -](?:[\w-]+\s+){0,3}(?:files?|outputs?)\b|\bBUILD\.bazel\b", re.I)
_MANUAL = re.compile(r"\b(?:hand[ -]edit\w*|manual(?:ly)?|by hand)\b", re.I)
_FREEZE = re.compile(
    r"\b(?:do not|don't|don’t|never|must not|no)\s+(?:change|modify|edit|regenerate|touch)\b|"
    r"\b(?:keep|leave|remain|stay|preserve)\b.*\b(?:unchanged|untouched|unmodified)\b|"
    r"\bmust\s+(?:be|remain|stay)\s+(?:unchanged|untouched|unmodified)\b", re.I)


def output_freezes(text):
    # Split sentences without splitting a file extension such as BUILD.bazel.
    return [part.strip() for part in re.split(r"(?<=[.!?])\s+|\n", text)
            if _OUTPUT.search(part) and _FREEZE.search(part) and not _MANUAL.search(part)]


def audit_generation_scope(inputs, reply):
    if not any(capability["tool"] == "run_generator" and capability["executor"] == "prismabuild_generator"
               for capability in inputs.get("tool_capabilities", ())):
        return
    issue = inputs["issue"]
    source = [("issue.title", issue.get("title", "")), ("issue.body", issue.get("body", ""))]
    conflicts = [f'Configured generation conflicts with {field}: "{quote}". '
                 'Resolve this explicit generated-output prohibition before planning; '
                 'manual-edit restrictions alone do not prohibit configured generation.'
                 for field, text in source for quote in output_freezes(text)]
    ingest = inputs["ingest"]
    if ingest["kind"] == "brief" and not conflicts:
        claims = [ingest["brief"], *(goal["description"] for goal in ingest["goals"])]
        conflicts = [f'Invented generated-output prohibition in ingest: "{quote}". '
                     'The source issue restricts model edits, not trusted configured generation. '
                     'Remove this added constraint and preserve the source wording.'
                     for claim in claims for quote in output_freezes(claim)]
    if conflicts:
        reply["verdict"] = "disagree"
        reply["objections"] = list(dict.fromkeys([*reply["objections"], *conflicts]))
