#!/usr/bin/env python3
"""Record a sandbox trial with REST GETs, or verify recorded trial snapshots."""

import argparse
import json
from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parent
REPOSITORY = "RobTand/tl3-sandbox"


def successful(snapshot, name, app_id, head, title=None):
    checks = [check for check in snapshot["head-checks"]
              if check["name"] == name and (check.get("app") or {}).get("id") == app_id
              and check["head_sha"] == head]
    if not checks:
        return False
    latest = max(checks, key=lambda check: check["id"])
    return (latest["status"] == "completed" and latest["conclusion"] == "success"
            and (title is None or (latest.get("output") or {}).get("title") == title))


def verify(before, draft, after, advanced=None):
    errors = []
    original = before["pull"]
    head = original["head"]["sha"]
    number = original["number"]
    for stage, snapshot in [("after", after)] + ([] if advanced is None else [("advanced", advanced)]):
        pull = snapshot["pull"]
        if pull["number"] != number:
            errors.append(f"{stage}: original PR changed")
        if pull["head"] != original["head"]:
            errors.append(f"{stage}: approved head changed")
    if original["merged"]:
        errors.append("before: original already merged")
    if not after["pull"]["merged"] or not after["pull"].get("merge_commit_sha"):
        errors.append("after: original not merged")
    for stage, snapshot in (("before", before), ("after", after)):
        if not successful(snapshot, "TL3 approval", 5542171, head):
            errors.append(f"{stage}: exact-head TL3 approval missing")
        if not successful(snapshot, "Sandbox tests", 15368, head):
            errors.append(f"{stage}: original CI missing")
    if not successful(after, "Mergify Merge Queue", 10562, head, "Merged via merge queue"):
        errors.append("after: successful queue merge evidence missing")
    temporary = draft["pull"]
    if (temporary["number"] == number
            or temporary["user"]["login"] != "mergify[bot]"
            or not temporary["head"]["ref"].startswith("mergify/merge-queue/")
            or temporary["head"]["sha"] == head):
        errors.append("draft: separate Mergify temporary draft missing")
    if not successful(draft, "Sandbox tests", 15368, temporary["head"]["sha"]):
        errors.append("draft: successful draft CI missing")
    if advanced is not None:
        if advanced["main"] == before["main"]:
            errors.append("advanced: main did not advance")
        if advanced["pull"]["merged"]:
            errors.append("advanced: original already merged")
    return errors


def record(number, destination):
    # Read main before the PR so an unmerged PR snapshot follows the advancement.
    main = json.loads(subprocess.check_output([
        "gh", "api", "--method", "GET", f"repos/{REPOSITORY}/git/ref/heads/main",
    ], text=True))
    subprocess.run([
        sys.executable, str(ROOT.parent.parent / "crates/mergify/tools/record.py"),
        REPOSITORY, str(number), str(destination),
    ], check=True)
    (destination / "main.json").write_text(json.dumps(main, indent=2) + "\n")


def load_snapshot(directory):
    def load(name):
        return json.loads((directory / f"{name}.json").read_text())
    if load("source")["repository"] != REPOSITORY:
        raise ValueError("recording is not from the sandbox")
    return {"pull": load("pull"), "head-checks": load("head-checks"),
            "main": load("main")["object"]["sha"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    capture = commands.add_parser("record")
    capture.add_argument("number", type=int)
    capture.add_argument("destination", type=Path)
    check = commands.add_parser("verify")
    for name in ("before", "draft", "after"):
        check.add_argument(name, type=Path)
    check.add_argument("--advanced", type=Path)
    args = parser.parse_args()
    try:
        if args.command == "record":
            record(args.number, args.destination)
            return 0
        errors = verify(load_snapshot(args.before), load_snapshot(args.draft),
                        load_snapshot(args.after),
                        None if args.advanced is None else load_snapshot(args.advanced))
        for error in errors:
            print(error)
        if errors:
            return 1
        print("PASS: recorded queue merge preserves the approved head")
        return 0
    except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError):
        print("Trial evidence unavailable or incomplete.", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
