"""Trusted outer entry point, already inside an admitted PB action."""
import json
from pathlib import Path
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent / "trusted"))
from tl3_harness import Check, Container, Forcing, HelperEvidence, HarnessInputError, Kind, run_checks
from tl3_harness.execution import require_admission, result_json
from tl3_harness.trees import git


def decode(request):
    """TL3's own check input. A malformed one is TL3's error (Q51)."""
    try:
        plan = tuple(Check(check["name"], Kind(check["kind"]), tuple(check["tests"]),
                           None if check.get("forcing") is None else Forcing.parse(check["forcing"]),
                           None if check.get("helper_evidence") is None else HelperEvidence.parse(check["helper_evidence"]),
                           check.get("declared_symptom"))
                     for check in request["plan"])
        protected = request.get("protected_paths", [])
        if not isinstance(protected, list) or not all(isinstance(path, str) and path for path in protected):
            raise ValueError("protected_paths must list path patterns")
        selected = request.get("selected_tests")
        if selected is not None and (type(selected) is not int or selected < 1):
            raise ValueError("selected_tests must be a positive limit")
        always = request.get("always_tests", [])
        if not isinstance(always, list):
            raise ValueError("always_tests must list pytest files or node IDs")
        return (plan, Container(request["image"]),
                tuple(protected), selected, tuple(always))
    except (KeyError, TypeError, ValueError) as error:
        raise HarnessInputError(f"invalid check request: {type(error).__name__}") from error


require_admission()
root = Path(__file__).resolve().parent
request = json.loads((root / "request.json").read_text())
with tempfile.TemporaryDirectory(prefix="tl3-controller-checks-") as directory:
    repo = Path(directory) / "served"
    git(root, "clone", "--quiet", "--template=", "--", str(root / "served.bundle"), str(repo))
    git(repo, "checkout", "--quiet", "--detach", request["head"])
    try:
        plan, container, protected, selected, always = decode(request)
        results = run_checks(repo, request["base"], request["head"], plan, container=container,
                             reviewed=request.get("reviewed"), protected=protected, selected_tests=selected,
                             always_tests=always, selected_failures=request.get("selected_failures", []))
    except HarnessInputError as error:
        # The trusted harness rejected TL3's own check input. Repository configuration,
        # admission and container failures stay upstream.
        print("TL3_CHECKS_ERROR " + json.dumps({"schema": "tl3.checks.error.v1", "effect_key": request["effect_key"],
              "error": type(error).__name__}, sort_keys=True, separators=(',', ':')), flush=True)
        raise
    report = {"schema": "tl3.checks.v1", "effect_key": request["effect_key"],
              "base": request["base"], "head": request["head"],
              "reviewed": request.get("reviewed"),
              "results": json.loads(result_json(results))}
    print("TL3_CHECKS " + json.dumps(report, sort_keys=True, separators=(',', ':')), flush=True)
