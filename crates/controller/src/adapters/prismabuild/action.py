"""Trusted outer entry point, already inside an admitted PB action."""
import json
from pathlib import Path
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent / "trusted"))
from tl3_harness import Check, Container, Kind, run_checks
from tl3_harness.execution import require_admission, result_json
from tl3_harness.trees import git

require_admission()
root = Path(__file__).resolve().parent
request = json.loads((root / "request.json").read_text())
with tempfile.TemporaryDirectory(prefix="tl3-controller-checks-") as directory:
    repo = Path(directory) / "served"
    git(root, "clone", "--quiet", "--template=", "--", str(root / "served.bundle"), str(repo))
    git(repo, "checkout", "--quiet", "--detach", request["head"])
    plan = tuple(Check(check["name"], Kind(check["kind"]), tuple(check["tests"]))
                 for check in request["plan"])
    results = run_checks(repo, request["base"], request["head"], plan,
                         container=Container(request["image"]), reviewed=request.get("reviewed"))
    report = {"schema": "tl3.checks.v1", "effect_key": request["effect_key"],
              "base": request["base"], "head": request["head"],
              "reviewed": request.get("reviewed"),
              "results": json.loads(result_json(results))}
    print("TL3_CHECKS " + json.dumps(report, sort_keys=True, separators=(',', ':')), flush=True)
