"""Inventory a served repository through run_checks in an admitted PB action."""
import argparse
from collections import Counter
import json
from pathlib import Path
import subprocess
import tempfile

from tl3_harness import Check, Container, Kind, run_checks
from tl3_harness import selection
from tl3_harness.execution import result_json


def node_ids(repo, files):
    """Use function selectors so pytest expands each parametrized case itself."""
    return [node for name in files for node in selection.node_ids(repo, name)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--per-file", action="store_true",
                        help="call run_checks separately for each selected test file")
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
        batches = ((check,) for check in checks) if args.per_file else (checks,)
        results = tuple(result for batch in batches
                        for result in run_checks(repo, revision, revision, batch,
                                                 container=Container(args.image)))
        print(json.dumps({"commit": revision, "image": args.image,
                          "counts": dict(Counter(test.head for result in results for test in result.tests)),
                          "results": json.loads(result_json(results))}), flush=True)
        return int(any(not test.passed for result in results for test in result.tests))


if __name__ == "__main__":
    raise SystemExit(main())
