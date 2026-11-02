"""Report plan measurements and rule violations to the controller."""

from pathlib import PurePosixPath
import re

from tl3_workers.roles.base import Role
from tl3_workers.forecast import Forecaster, HARD_CAP_BYTES
from tl3_harness import Check, Kind
from tl3_harness.limits import validate_check_plan
from tl3_harness.trees import test_function_source
from tl3_workers.tools import Worktree


def check_reply(inputs, reply):
    if "forecast" in reply or "violations" in reply:
        raise ValueError("only code may supply planning evidence")
    goals = set(range(len(inputs["ingest"]["goals"])))
    violations = []
    paths = set()
    if reply["kind"] == "leaf":
        if inputs["must_split"]:
            violations.append("Must split after running out of room.")
        if not reply["files"]:
            violations.append("A leaf must declare its files.")
        if not reply["no_design_questions"]:
            violations.append("A leaf must resolve every design question.")
        if not reply["lands_on_its_own"]:
            violations.append("A leaf must land on its own.")
        covered = set()
        for check in reply["checks"]:
            if not check["node_ids"]:
                violations.append("Every check must name at least one test.")
            if not check["goal_ids"]:
                violations.append("Every check must cite at least one goal.")
            for node_id in check["node_ids"]:
                try:
                    Check("planned", Kind(check["kind"]), (node_id,))
                except ValueError:
                    violations.append("Checks must name exact pytest node IDs.")
                    continue
                if check["kind"] in {"regression", "behavior-preserving"}:
                    path = node_id.split("::")[0]
                    oversized, exists = base_definition(node_id)
                    if oversized:
                        violations.append(f"Existing check's test file is over the {HARD_CAP_BYTES}-byte limit: {path}")
                    elif not exists:
                        violations.append(f"Existing check must name a test defined at the base: {node_id}")
            cited = set(check["goal_ids"])
            if not cited <= goals:
                violations.append("A check cites an unknown goal.")
            covered.update(cited)
        violations.extend(edit_violations(inputs["ingest"]["goals"], reply))
        if "Checks must name exact pytest node IDs." not in violations:
            try:
                validate_check_plan(tuple(Check(f'check-{i + 1}', Kind(check['kind']), tuple(check['node_ids']))
                                          for i, check in enumerate(reply['checks']) if check['node_ids']))
            except ValueError as error:
                violations.append(str(error))
        if covered != goals:
            violations.append("Every goal must have a check.")
        paths.update(reply["files"])
    elif reply["kind"] == "split":
        covered = set()
        for piece in reply["pieces"]:
            if not piece["goal_ids"]:
                violations.append("Every piece must cite at least one goal.")
            cited = set(piece["goal_ids"])
            if not cited <= goals:
                violations.append("A piece cites an unknown goal.")
            covered.update(cited)
            paths.update(piece["files"])
        if len(reply["pieces"]) < 2:
            violations.append("A split must propose at least two pieces.")
        if covered != goals:
            violations.append("A split must preserve every goal.")
    else:
        paths.update(reply["files"])

    if reply["kind"] in {"leaf", "split"}:
        for path in sorted(paths):
            if any(PurePosixPath(path).full_match(pattern) for pattern in inputs["protected_paths"]):
                violations.append(f"Declared file matches a protected path: {path}")

    forecaster = Forecaster(inputs["forecast"])
    forecast = forecaster.measure(inputs["ingest"]["brief"], paths)
    # A file the tools can't use is a plan problem, not a crash (#321).
    for error in forecaster.problems:
        violations.append(f"Declared file can't be used: {error}. Leave it out.")
    if reply["kind"] == "leaf" and not forecast["fits"]:
        violations.append("The leaf forecast does not fit. Propose smaller pieces.")
    if reply["kind"] == "split":
        reason = reply["reason"]
        if reason == "multiple_repos":
            violations.append("This request is in one repository.")
        if reason == "out_of_room" and not inputs["must_split"]:
            violations.append("No maker has run out of room.")
        if reason == "does_not_fit" and forecast["fits"]:
            violations.append("The forecast fits. Splitting needs a permitted reason.")
        for piece in reply["pieces"]:
            child = forecaster.measure(piece["brief"], piece["files"])
            for error in forecaster.problems:
                violations.append(f"Declared file can't be used: {error}. Leave it out.")
            if child["tokens"] >= forecast["tokens"]:
                violations.append("Every proposed piece must have a smaller forecast than its parent.")
    reply["forecast"] = forecast
    reply["violations"] = list(dict.fromkeys(violations))


def base_definition(node_id):
    """Whether the base defines the test, after whether its file is over the hard cap."""
    # The whole module is parsed to find the test, so the
    # declared-file hard cap bounds it, not a page size.
    try:
        with Worktree(".") as tree, tree._file(node_id.split("::")[0]) as stream:
            source = stream.read(HARD_CAP_BYTES + 1)
    except (OSError, ValueError, UnicodeError):
        return False, False
    if len(source.encode("utf-8")) > HARD_CAP_BYTES:
        return True, False
    return False, test_function_source(source, node_id) is not None


def function(node_id):
    # The harness's edit guard compares a test's function source, so every case shares it.
    return node_id.split("[", 1)[0]


def edit_violations(goals, reply):
    """Regression and behavior-preserving checks run tests exactly as the base wrote them,
    so a test the change edits can only be shown by a demonstrating check (#438)."""
    violations = []
    frozen, demonstrated = {}, set()
    for index, check in enumerate(reply["checks"]):
        for node_id in check["node_ids"]:
            if check["kind"] == "demonstrating":
                demonstrated.add(function(node_id))
            else:
                frozen.setdefault(function(node_id), (f"check-{index + 1}", check["kind"], node_id))
    edits = reply.get("edits_tests", [])
    edited = set()
    for node_id in edits:
        try:
            Check("edits", Kind.DEMONSTRATING, (node_id,))
        except ValueError:
            violations.append("edits_tests must name exact pytest node IDs.")
            continue
        edited.add(function(node_id))
        path = node_id.split("::")[0]
        if path not in reply["files"]:
            violations.append(f"A test the change edits must be in a declared file: `{path}`.")
        oversized, exists = base_definition(node_id)
        if oversized:
            violations.append(f"edits_tests names a test in a file over the {HARD_CAP_BYTES}-byte limit: {path}")
        elif not exists:
            violations.append(f"edits_tests must name a test defined at the base: `{node_id}`. "
                              "A test the change adds belongs only in a demonstrating check.")
        if function(node_id) in frozen:
            violations.append(f"`{node_id}` is in edits_tests, so a regression or behavior-preserving check "
                              "can't name it: those checks run tests exactly as the base wrote them. "
                              "Name it only in a demonstrating check.")
        if function(node_id) not in demonstrated:
            violations.append(f"`{node_id}` is in edits_tests but no demonstrating check names it. "
                              "Name it in a demonstrating check that fails on the base by an assertion.")
    tests = {node_id.split("::")[0] for check in reply["checks"] for node_id in check["node_ids"]}
    if edited and set(reply["files"]) <= tests | {node_id.split("::")[0] for node_id in edits}:
        violations.append("This leaf changes only tests, so no check can show it: the base run applies the "
                          "leaf's test edits, so base and head run the same test. Return can't meet and name "
                          "#393, which asks Rob whether a forced reproduction counts as deflake evidence.")
    # A heuristic: a goal that names a frozen test in a declared file probably edits it.
    for goal_id, goal in enumerate(goals):
        for key, (name, kind, node_id) in frozen.items():
            test = key.rsplit("::", 1)[-1]
            if (key not in edited and node_id.split("::")[0] in reply["files"]
                    and re.search(rf"(?<!\w){re.escape(test)}(?!\w)", goal["description"])):
                violations.append(f"Goal {goal_id} names `{test}`, and {name} freezes `{node_id}` as a {kind} "
                                  "check. If the change edits that test, list it in edits_tests and name it only "
                                  "in a demonstrating check. If it leaves the test as written there, guard that "
                                  "behavior with other existing tests.")
    return violations


ROLE = Role(
    name="planner",
    prompt="planner.md",
    input_schema="planner-input",
    reply_schema="planner-reply",
    code_only_fields=("forecast", "violations"),
    tools={
        "list": "List worktree directories to locate material for the plan.",
        "read": "Read worktree text to identify files and checks for the goals.",
        "glob": "Find worktree paths using relative glob patterns for planning. The path must be a directory.",
        "grep": "Find references in worktree text to check the plan. The pattern is an RE2 regular expression, with no backreferences or lookarounds, matched per line; set literal to true to search text as written. The path may be a file or a directory.",
    },
    check_reply=check_reply,
)
