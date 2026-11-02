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
    try:
        plan = tuple(Check(check["name"], Kind(check["kind"]), tuple(check["tests"]))
                     for check in request["plan"])
        results = run_checks(repo, request["base"], request["head"], plan,
                             container=Container(request["image"], exec_scratch=request.get("exec_scratch", False)), reviewed=request.get("reviewed"))
    except (ValueError, KeyError, TypeError) as error:
        # The trusted harness rejected TL3's own check input. That is TL3's error,
        # not PrismaBuild's (Q51). Admission and container failures stay upstream.
        print("TL3_CHECKS_ERROR " + json.dumps({"schema": "tl3.checks.error.v1", "effect_key": request["effect_key"],
              "error": type(error).__name__}, sort_keys=True, separators=(',', ':')), flush=True)
        raise
    report = {"schema": "tl3.checks.v1", "effect_key": request["effect_key"],
              "base": request["base"], "head": request["head"],
              "reviewed": request.get("reviewed"),
              "results": json.loads(result_json(results))}
    print("TL3_CHECKS " + json.dumps(report, sort_keys=True, separators=(',', ':')), flush=True)
