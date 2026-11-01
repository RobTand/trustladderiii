"""Inventory a served repository through run_checks in an admitted PB action."""
import argparse
import ast
from collections import Counter
import json
from pathlib import Path
import subprocess
import tempfile

from tl3_harness import Check, Container, Kind, run_checks
from tl3_harness.execution import result_json


def node_ids(repo, files):
    """Use function selectors so pytest expands each parametrized case itself."""
    nodes = []
    for name in files:
        module = ast.parse((repo / name).read_bytes())
        disabled = {target.value.id for statement in module.body
                    if isinstance(statement, ast.Assign) and isinstance(statement.value, ast.Constant)
                    and statement.value.value is False for target in statement.targets
                    if isinstance(target, ast.Attribute) and target.attr == "__test__"
                    and isinstance(target.value, ast.Name)}
        for node in module.body:
            if getattr(node, "name", None) in disabled:
                continue
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name.startswith("test_"):
                nodes.append(f"{name}::{node.name}")
            elif isinstance(node, ast.ClassDef) and node.name.startswith("Test"):
                nodes.extend(f"{name}::{node.name}::{method.name}" for method in node.body
                             if isinstance(method, (ast.FunctionDef, ast.AsyncFunctionDef))
                             and method.name.startswith("test_"))
    return nodes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bundle", type=Path)
    parser.add_argument("image")
    parser.add_argument("files", nargs="+")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="tl3-inventory-") as directory:
        repo = Path(directory) / "served"
        subprocess.run(["git", "clone", str(args.bundle.resolve()), str(repo)], check=True)
        revision = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
        nodes = node_ids(repo, args.files)
        checks = tuple(Check(name, Kind.BEHAVIOR_PRESERVING,
                             tuple(node for node in nodes if node.split("::")[0] == name))
                       for name in sorted({node.split("::")[0] for node in nodes}))
        results = run_checks(repo, revision, revision, checks, container=Container(args.image))
        print(json.dumps({"commit": revision, "image": args.image,
                          "counts": dict(Counter(test.head for result in results for test in result.tests)),
                          "results": json.loads(result_json(results))}), flush=True)
        return int(any(not test.passed for result in results for test in result.tests))


if __name__ == "__main__":
    raise SystemExit(main())
