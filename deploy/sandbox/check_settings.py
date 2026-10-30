#!/usr/bin/env python3
"""Compare the sandbox's REST settings and gate files with the reviewed baseline."""

import argparse
import base64
import hashlib
import json
from pathlib import Path
import subprocess
import sys
from urllib.parse import quote


ROOT = Path(__file__).resolve().parent
REPOSITORY = "RobTand/tl3-sandbox"
FILES = (".mergify.yml", ".github/workflows/tests.yml", ".github/CODEOWNERS")
SETTINGS = (
    "default_branch", "visibility", "allow_merge_commit", "allow_squash_merge",
    "allow_rebase_merge", "allow_auto_merge", "allow_update_branch",
    "delete_branch_on_merge", "merge_commit_title", "merge_commit_message",
)
VOLATILE = {
    "url", "_links",
}


class ReadError(Exception):
    pass


class GitHub:
    def get(self, endpoint, *, missing=False):
        result = subprocess.run(
            ["gh", "api", "--method", "GET", "--include",
             "-H", "Accept: application/vnd.github+json",
             "-H", "X-GitHub-Api-Version: 2022-11-28", endpoint],
            text=True, capture_output=True, check=False,
        )
        # Do not print gh's stderr, response headers or arbitrary error bodies.
        header, separator, body = result.stdout.partition("\n\n")
        try:
            status = int(header.splitlines()[0].split()[1])
        except (IndexError, ValueError):
            raise ReadError("REST response unavailable") from None
        if status == 404 and missing:
            return None
        if result.returncode or status != 200 or not separator:
            raise ReadError(f"REST read failed, HTTP {status}")
        try:
            return json.loads(body)
        except ValueError:
            raise ReadError("REST response is not JSON") from None

    def pages(self, endpoint):
        values = []
        for page in range(1, 1001):
            batch = self.get(f"{endpoint}?per_page=100&page={page}")
            if not isinstance(batch, list):
                raise ReadError("REST list response is not an array")
            values.extend(batch)
            if len(batch) < 100:
                return values
        raise ReadError("REST pagination did not terminate")


def normalize(value):
    if isinstance(value, dict):
        return {key: normalize(item) for key, item in value.items()
                if key not in VOLATILE}
    if isinstance(value, list):
        return sorted((normalize(item) for item in value),
                      key=lambda item: json.dumps(item, sort_keys=True))
    return value


def differences(expected, actual, path=""):
    """Return every mismatch; missing values differ from explicit nulls."""
    result = []
    if type(expected) is not type(actual):
        return [f"{path}: expected {json.dumps(expected)}, got {json.dumps(actual)}"]
    if isinstance(expected, dict):
        for key in sorted(expected.keys() | actual.keys()):
            child = f"{path}.{key}" if path else key
            if key not in actual:
                result.append(f"{child}: missing")
            elif key not in expected:
                result.append(f"{child}: unexpected {json.dumps(actual[key], sort_keys=True)}")
            else:
                result.extend(differences(expected[key], actual[key], child))
    elif isinstance(expected, list):
        for index in range(max(len(expected), len(actual))):
            child = f"{path}[{index}]"
            if index >= len(actual):
                result.append(f"{child}: missing {json.dumps(expected[index], sort_keys=True)}")
            elif index >= len(expected):
                result.append(f"{child}: unexpected {json.dumps(actual[index], sort_keys=True)}")
            else:
                result.extend(differences(expected[index], actual[index], child))
    elif expected != actual:
        result.append(f"{path}: expected {json.dumps(expected)}, got {json.dumps(actual)}")
    return result


def collect(github):
    prefix = f"repos/{REPOSITORY}"
    actual = {}
    errors = []

    def read(name, operation):
        try:
            actual[name] = operation()
        except (ReadError, KeyError, TypeError, ValueError):
            errors.append(f"{name}: could not read complete REST settings")

    # Fetch repository settings once, rather than once per field.
    def settings():
        repo = github.get(prefix)
        return {key: repo[key] for key in SETTINGS}
    read("repository", settings)

    try:
        summaries = github.pages(f"{prefix}/rulesets")
        actual["rulesets"] = []
        for ruleset in summaries:
            try:
                detail = github.get(f"{prefix}/rulesets/{ruleset['id']}")
                actual["rulesets"].append(normalize({
                    key: value for key, value in detail.items()
                    if key not in {"id", "node_id", "created_at", "updated_at",
                                   "current_user_can_bypass", "source", "source_type"}
                }))
            except (ReadError, KeyError, TypeError, ValueError):
                errors.append("rulesets: could not read a complete ruleset")
        actual["rulesets"] = normalize(actual["rulesets"])
    except (ReadError, KeyError, TypeError, ValueError):
        errors.append("rulesets: could not list REST rulesets")

    actual["branch_protection"] = {}
    try:
        branches = github.pages(f"{prefix}/branches")
        names = {"main"} | {b["name"] for b in branches if b["protected"]}
        for branch in sorted(names):
            try:
                protection = github.get(
                    f"{prefix}/branches/{quote(branch, safe='')}/protection", missing=True)
                if branch == "main" or protection is not None:
                    actual["branch_protection"][branch] = normalize(protection)
            except (ReadError, KeyError, TypeError, ValueError):
                errors.append(f"branch_protection.{branch}: REST read failed")
    except (ReadError, KeyError, TypeError, ValueError):
        errors.append("branch_protection: could not list REST branches")

    actual["files"] = {}
    for path in FILES:
        try:
            file = github.get(f"{prefix}/contents/{path}?ref=main", missing=True)
            if file is None:
                actual["files"][path] = None
            else:
                if file["encoding"] != "base64" or file["type"] != "file":
                    raise ReadError("unsupported file response")
                data = base64.b64decode("".join(file["content"].split()), validate=True)
                actual["files"][path] = hashlib.sha256(data).hexdigest()
        except (ReadError, KeyError, TypeError, ValueError):
            errors.append(f"files.{path}: could not read REST content")
    return actual, errors


def check(approved, github, files=ROOT / "files"):
    actual, errors = collect(github)
    result = errors + differences(normalize(approved), normalize(actual))
    for path in FILES:
        try:
            digest = hashlib.sha256((files / path).read_bytes()).hexdigest()
            result.extend(differences(approved["files"][path], digest, f"local_files.{path}"))
        except (OSError, KeyError):
            result.append(f"local_files.{path}: approved copy unavailable")
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--approved", type=Path, default=ROOT / "approved.json")
    args = parser.parse_args(argv)
    try:
        approved = json.loads(args.approved.read_text())
        if set(approved) != {"repository", "rulesets", "branch_protection", "files"}:
            raise ValueError("invalid baseline")
        if set(approved["files"]) != set(FILES) or set(approved["repository"]) != set(SETTINGS):
            raise ValueError("incomplete baseline")
        changes = check(approved, GitHub())
    except (OSError, ValueError, TypeError):
        print("Settings check unavailable: review the baseline and REST tooling.", file=sys.stderr)
        return 2
    for change in changes:
        print(change)
    if changes:
        print(f"FAIL: {len(changes)} difference(s) or incomplete reads")
        return 1
    print(f"PASS: {REPOSITORY} matches the approved sandbox gate")
    return 0


if __name__ == "__main__":
    sys.exit(main())
