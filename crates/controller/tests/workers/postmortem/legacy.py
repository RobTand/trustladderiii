"""Pre-#782 semantic oracle, copied verbatim for migration parity."""
import json
import sys
MAX_RECOMMENDATIONS = 3

def check_reply(inputs, reply):
    recommendations = reply["postmortem"]["recommendations"]
    if len(recommendations) > MAX_RECOMMENDATIONS:
        raise ValueError("at most three recommendations")
    for item in recommendations:
        before, after = item["before"], item["after"]
        if (item["operation"] == "add" and (before or not after)) or (
                item["operation"] == "remove" and (after or not before)) or (
                item["operation"] == "replace" and not (before and after)):
            raise ValueError("a recommendation's before and after must fit its operation")

results = []
for case in json.load(sys.stdin):
    try:
        check_reply(case["inputs"], case["reply"])
        results.append([])
    except ValueError as error:
        results.append([str(error)])
print(json.dumps(results, ensure_ascii=False))
