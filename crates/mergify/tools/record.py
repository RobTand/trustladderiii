#!/usr/bin/env python3
"""Record public queue observations. Makes only GitHub REST GET requests."""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import subprocess


def get(endpoint):
    return json.loads(subprocess.check_output(["gh", "api", endpoint]))


def fields(value, names):
    return {name: value.get(name) for name in names.split()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repo")
    parser.add_argument("number", type=int)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    prefix = f"repos/{args.repo}"
    pull = get(f"{prefix}/pulls/{args.number}")
    checks = []
    comments = []
    for endpoint, target, key in [
        (f"commits/{pull['head']['sha']}/check-runs?filter=all", checks, "check_runs"),
        (f"issues/{args.number}/comments?", comments, None),
    ]:
        for page in range(1, 1001):
            value = get(f"{prefix}/{endpoint}&per_page=100&page={page}")
            values = value[key] if key else value
            target.extend(values)
            if len(values) < 100:
                break
        else:
            raise RuntimeError("pagination did not terminate")
    head_checks = checks
    checks = [fields(c, "id name head_sha external_id status conclusion app output")
              for c in checks if c.get("app", {}).get("slug") == "mergify"]
    for check in checks:
        check["app"] = fields(check["app"], "id slug")
        if check["output"]:
            check["output"] = fields(check["output"], "title summary text")
    comments = [fields(c, "id body user created_at updated_at issue_url")
                for c in comments if c["user"]["login"] == "mergify[bot]"]
    for comment in comments:
        comment["user"] = fields(comment["user"], "id login")
    suites = get(f"{prefix}/commits/{pull['head']['sha']}/check-suites?per_page=100")
    if suites["total_count"] > len(suites["check_suites"]):
        raise RuntimeError("check suite recording requires pagination")
    events = []
    for page in range(1, 1001):
        batch = get(f"{prefix}/issues/{args.number}/events?per_page=100&page={page}")
        events.extend(batch)
        if len(batch) < 100:
            break
    else:
        raise RuntimeError("event pagination did not terminate")
    events = [fields(event, "id event actor label created_at") for event in events]
    for event in events:
        if event["actor"]:
            event["actor"] = fields(event["actor"], "id login")
    merge_checks = None
    if pull.get("merged_at") and pull.get("merge_commit_sha"):
        merge_checks = get(f"{prefix}/commits/{pull['merge_commit_sha']}/check-runs?filter=all&per_page=100")
        if merge_checks["total_count"] > len(merge_checks["check_runs"]):
            raise RuntimeError("merge check recording requires pagination")
    pull = fields(pull, "number user state merged merged_at merge_commit_sha head base updated_at")
    pull["user"] = fields(pull["user"], "id login")
    for name in ["head", "base"]:
        pull[name] = fields(pull[name], "ref sha repo")
        repo = pull[name]["repo"]
        if repo:
            pull[name]["repo"] = fields(repo, "id name owner")
            pull[name]["repo"]["owner"] = fields(repo["owner"], "id login")
    args.destination.mkdir(parents=True, exist_ok=False)
    def check_fields(check):
        result = fields(check, "id name head_sha external_id status conclusion app output")
        if result["app"]:
            result["app"] = fields(result["app"], "id slug")
        if result["output"]:
            result["output"] = fields(result["output"], "title summary text")
        return result

    suite_records = []
    for suite in suites["check_suites"]:
        record = fields(suite, "id head_sha status conclusion app")
        if record["app"]:
            record["app"] = fields(record["app"], "id slug")
        suite_records.append(record)
    for name, value in [
        ("pull", pull), ("checks", checks), ("comments", comments),
        ("head-checks", [check_fields(check) for check in head_checks]),
        ("suites", suite_records), ("events", events),
        ("merge-checks", None if merge_checks is None else
         [check_fields(check) for check in merge_checks["check_runs"]]),
    ]:
        (args.destination / f"{name}.json").write_text(json.dumps(value, indent=2) + "\n")
    (args.destination / "source.json").write_text(json.dumps({
        "repository": args.repo, "pull_request": args.number,
        "recorded_at": datetime.now(timezone.utc).isoformat(),
        "head": pull["head"]["sha"],
    }, indent=2) + "\n")


if __name__ == "__main__":
    main()
