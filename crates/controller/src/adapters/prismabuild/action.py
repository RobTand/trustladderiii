"""Trusted outer entry point, already inside an admitted PB action."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent / "trusted"))
from tl3_harness import Check, Container, Kind, run_checks
from tl3_harness.execution import require_admission, result_json

require_admission()
root = Path(__file__).resolve().parent
request = json.loads((root / "request.json").read_text())
with tempfile.TemporaryDirectory(prefix="tl3-controller-checks-") as directory:
    repo = Path(directory) / "served"
    subprocess.run(["git", "-c", "core.hooksPath=/dev/null", "clone", "--quiet", "--template=",
                    str(root / "served.bundle"), str(repo)], check=True)
    subprocess.run(["git", "-C", str(repo), "-c", "core.hooksPath=/dev/null", "checkout",
                    "--quiet", "--detach", request["head"]], check=True)
    plan = tuple(Check(check["name"], Kind(check["kind"]), tuple(check["tests"]))
                 for check in request["plan"])
    results = run_checks(repo, request["base"], request["head"], plan,
                         container=Container(request["image"]))
    report = {"schema": "tl3.checks.v1", "effect_key": request["effect_key"],
              "base": request["base"], "head": request["head"],
              "results": json.loads(result_json(results))}
    print("TL3_CHECKS " + json.dumps(report, sort_keys=True), flush=True)
