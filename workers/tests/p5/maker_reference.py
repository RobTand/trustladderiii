"""P5 pre-extraction reference from cdacce8f99; remove after integration."""
from tl3_workers.tools import _parts


def _check_inputs(inputs):
    goals = [goal["id"] for goal in inputs["goals"]]
    if len(set(goals)) != len(goals):
        raise ValueError("goal ids must be unique")
    for path in inputs["plan"]["files"]:
        if not _parts(path):
            raise ValueError("expected a declared file")
    for item in [*inputs["plan"]["checks"], *inputs.get("feedback", [])]:
        if not set(item["goal_ids"]) <= set(goals):
            raise ValueError("unknown goal id")
    required = {goal["id"] for goal in inputs["goals"] if goal.get("stage", "maker") != "publishing"}
    covered = {goal for check in inputs["plan"]["checks"] for goal in check["goal_ids"]}
    if not required <= covered:
        raise ValueError("checks must cover every maker and checking goal")
