"""Report plan measurements and rule violations to the controller."""

from pathlib import PurePosixPath

from tl3_workers.roles.base import Role
from tl3_workers.forecast import Forecaster
from tl3_harness import Check, Kind


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
            cited = set(check["goal_ids"])
            if not cited <= goals:
                violations.append("A check cites an unknown goal.")
            covered.update(cited)
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
            if child["tokens"] >= forecast["tokens"]:
                violations.append("Every proposed piece must have a smaller forecast than its parent.")
    reply["forecast"] = forecast
    reply["violations"] = list(dict.fromkeys(violations))


ROLE = Role(
    name="planner",
    prompt="planner.md",
    input_schema="planner-input",
    reply_schema="planner-reply",
    code_only_fields=("forecast", "violations"),
    tools={
        "list": "List worktree directories to locate material for the plan.",
        "read": "Read worktree text to identify files and checks for the goals.",
        "glob": "Find worktree paths using relative glob patterns for planning.",
        "grep": "Find literal references in worktree text to check the plan.",
    },
    check_reply=check_reply,
)
