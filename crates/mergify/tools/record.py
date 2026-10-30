#!/usr/bin/env python3
"""Record public queue observations. Makes only GitHub REST GET requests."""
import argparse
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
    pull = fields(pull, "number state merged_at head base updated_at")
    for name in ["head", "base"]:
        pull[name] = fields(pull[name], "ref sha repo")
        repo = pull[name]["repo"]
        if repo:
            pull[name]["repo"] = fields(repo, "id name owner")
            pull[name]["repo"]["owner"] = fields(repo["owner"], "id login")
    args.destination.mkdir(parents=True, exist_ok=False)
    for name, value in [("pull", pull), ("checks", checks), ("comments", comments)]:
        (args.destination / f"{name}.json").write_text(json.dumps(value, indent=2) + "\n")
    (args.destination / "source.json").write_text(json.dumps({
        "repository": args.repo, "pull_request": args.number,
        "head": pull["head"]["sha"],
    }, indent=2) + "\n")


if __name__ == "__main__":
    main()
