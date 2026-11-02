"""Count maker cant_meet replies whose next steps name only the maker's own tools (#434).

Usage: cant_meet_audit.py <traces.json> <artifact-dir>

traces.json is the REST list of traces (`{"traces": [...]}`) for experiment 2, role maker. The
artifact directory holds `<request_id>.json` from ajax-api/2.0/mlflow/get-trace-artifact. Download
both into a fresh directory, and run this with `python3 -I`. Run it on the same query before and
after a change and compare the counts. The text rules are a screen: read the printed reports.
"""
import json
from pathlib import Path
import re
import sys

# Where a report lists what remains.
NEXT_STEPS = re.compile(
    r"NEXT STEPS|Remaining work|SUGGESTED ORDER|Suggested completion|To finish\b|FOR THE LEAD"
    r"|What would resolve|would resolve (?:it|this)|Before this can pass|ways forward", re.I)
# A step only the controller, the lead or a person can take.
NEEDS_OTHERS = re.compile(
    r"\b(?:controller|lead|waive[sd]?|accept|re-?plan|re-?type|supply|provide|decide|direction|"
    r"footprint recheck|Rob)\b|rule conflict|who can", re.I)
# A step the maker's own tools can take: read, grep, diff, edit, write, run_tests, recall.
OWN_TOOLS = re.compile(r"\b(?:grep|read|diff|edit|write|rerun|re-run|run|restore|update|fix|add|tighten|recall)\b", re.I)
MIN_REPORT = 60


def classify(report):
    """One of: no-next-steps, tool-only, needs-others, mixed."""
    if len(report) < MIN_REPORT:
        return "no-next-steps"
    found = list(NEXT_STEPS.finditer(report))
    steps = report[found[-1].start():] if found else report
    others = bool(NEEDS_OTHERS.search(steps))
    own = bool(OWN_TOOLS.search(steps))
    if others and own:
        return "mixed"
    if others:
        return "needs-others"
    return "tool-only" if own and found else "no-next-steps"


NO_DIFF = re.compile(r"(?i)\b(?:no|without|have no) (?:git )?diff (?:tool|access)|\b(?:cannot|can't|could not|couldn't) diff\b")


def mentions_no_diff(report):
    return NO_DIFF.search(report)


def replies(traces, artifacts):
    for trace in traces:
        spans = json.loads((Path(artifacts) / f"{trace['request_id']}.json").read_text())["spans"]
        root = next(span for span in spans if not span["parent_span_id"])
        output = root["attributes"].get("mlflow.spanOutputs")
        try:
            reply = json.loads(output) if output else None
            reply = json.loads(reply) if isinstance(reply, str) else reply
        except ValueError:
            continue
        if isinstance(reply, dict) and reply.get("kind"):
            yield trace, reply


def main(argv):
    traces = json.loads(Path(argv[1]).read_text())["traces"]
    kinds, classes, no_diff = {}, {}, 0
    for trace, reply in replies(traces, argv[2]):
        kinds[reply["kind"]] = kinds.get(reply["kind"], 0) + 1
        if reply["kind"] != "cant_meet":
            continue
        label = classify(reply["report"])
        classes[label] = classes.get(label, 0) + 1
        said = mentions_no_diff(reply["report"])
        no_diff += bool(said)
        print(f"{trace['request_id'][:11]} {label:14} {len(reply['report']):5} chars" + (f"  no diff: ...{said.group(0)}" if said else ""))
    print("replies by kind:", kinds)
    print("cant_meet by next steps:", classes)
    print("cant_meet that say there was no diff:", no_diff)


if __name__ == "__main__":
    main(sys.argv)
