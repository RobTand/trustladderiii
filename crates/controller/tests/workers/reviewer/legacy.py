"""Temporary #781 parity oracle, copied verbatim from the integration base."""
import json
import sys

def check_reply(inputs, reply):
    if reply["verdict"] == "approve" and reply["head_sha"] != inputs["head_sha"]:
        raise ValueError("approval names a head other than the reviewed head")
    if reply["verdict"] == "out_of_room":
        return
    known = {
        "goal": {goal["id"] for goal in inputs["goals"]},
        "standing_rule": {rule["id"] for rule in inputs["standing_rules"]},
    }
    for finding in reply["blocking_findings"]:
        citation = finding["citation"]
        if citation["id"] not in known[citation["kind"]]:
            raise ValueError("blocking finding cites an unknown goal or standing rule")
        if citation == {"kind": "standing_rule", "id": "request-scope"}:
            changed = (line for line in inputs.get("scope_diff", inputs["diff"]).splitlines()
                       if line.startswith(("+", "-"))
                       and not line.startswith(("+++", "---"))
                       and sum(not char.isspace() for char in line[1:]) >= 8)
            quoted = {line.strip() for line in finding["evidence"].splitlines()}
            if not any(line in quoted for line in changed):
                raise ValueError("scope blocker must quote a changed hunk")


cases = json.load(sys.stdin)
results = []
for case in cases:
    try:
        check_reply(case["inputs"], case["reply"])
        results.append([])
    except ValueError as error:
        results.append([str(error)])
json.dump(results, sys.stdout)
