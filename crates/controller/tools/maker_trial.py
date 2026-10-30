"""Run the real maker, worktrees adapter and harness in one admitted action."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

from tl3_harness.execution import Container, require_admission, result_json, run_checks
from tl3_harness.model import Check, Kind, Verdict


def main():
    require_admission()
    image = Container(sys.argv[1])
    with tempfile.TemporaryDirectory(prefix="tl3-maker-trial-") as directory:
        root = Path(directory)
        root.chmod(0o755)
        subprocess.run(["cargo", "test", "-p", "tl3-controller", "--test", "workers", "--locked",
                        "--", "--maker-trial", str(root)], check=True)
        trial = json.loads((root / "maker-trial.json").read_text())
        results = run_checks(Path(trial["repository"]), trial["base"], trial["head"],
                             (Check("hello", Kind.DEMONSTRATING,
                                    ("test_greeting.py::test_hello",)),), container=image)
        assert results[0].verdict == Verdict.PASS
        assert trial["changed_paths"] == ["greeting.py"]
        failures = run_checks(Path(trial["repository"]), trial["base"], trial["base"],
                              (Check("hello", Kind.REGRESSION,
                                     ("test_greeting.py::test_hello",)),), container=image)
        assert failures[0].verdict == Verdict.FAIL
        assert "goodbye" in failures[0].tests[0].message
        trial.pop("repository")
        trial.pop("remote")
        print(json.dumps({"maker_trial": trial, "report": json.loads(result_json(results)),
                          "failure_report": json.loads(result_json(failures)),
                          "image": image.image, "verification": "passed"}), flush=True)


if __name__ == "__main__":
    main()
