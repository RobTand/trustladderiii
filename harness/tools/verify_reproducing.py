"""Fixture repos for review fixes, run inside an admitted PrismaBuild action."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

from tl3_harness import Check, Container, Kind, Verdict, run_checks
from tl3_harness.execution import result_json
from tl3_harness.trees import git


def save(repo, message):
    git(repo, "add", "--all")
    git(repo, "add", "--renormalize", ".")
    git(repo, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
        "commit", "-qm", message)
    return git(repo, "rev-parse", "HEAD").decode().strip()


def verify_action_reviewed(repo, base, reviewed, head, container):
    with tempfile.TemporaryDirectory(prefix="tl3-action-fixture-") as directory:
        root = Path(directory)
        source = Path(__file__).resolve().parents[2]
        shutil.copytree(source / "harness/src/tl3_harness", root / "trusted/tl3_harness")
        shutil.copyfile(source / "crates/controller/src/adapters/prismabuild/action.py", root / "action.py")
        git(repo, "bundle", "create", str(root / "served.bundle"), "HEAD")
        request = {"effect_key": "fixture-reviewed", "base": base, "reviewed": reviewed,
                   "head": head, "image": container.image,
                   "plan": [{"name": "round", "kind": "reproducing",
                             "tests": ["test_value.py::test_reproduces"]}]}
        (root / "request.json").write_text(json.dumps(request))
        child = subprocess.run([sys.executable, str(root / "action.py")], capture_output=True,
                               text=True, env=os.environ)
        assert child.returncode == 0, child.stderr
        report = json.loads(next(line.removeprefix("TL3_CHECKS ")
                                 for line in child.stdout.splitlines() if line.startswith("TL3_CHECKS ")))
        assert report["reviewed"] == reviewed
        assert report["results"][0]["verdict"] == "pass", report
        assert report["results"][0]["tests"][0]["base"] == "assertion"
        print("action.py reviewed request passed", flush=True)


def verify(container):
    kind = next((kind for kind in Kind if kind.value == "reproducing"), None)
    assert kind is not None, "review fixes need reproducing checks on the reviewed commit"
    with tempfile.TemporaryDirectory(prefix="tl3-reproducing-") as directory:
        repo = Path(directory)
        repo.chmod(0o755)
        git(repo, "init", "-q")
        (repo / "pyproject.toml").write_text(
            '[tool.tl3.harness]\ncommand = ["python", "-m", "pytest"]\ntest-files = ["test_*.py"]\n')
        (repo / "product.py").write_text("def value(): return 2\n")
        (repo / "test_value.py").write_text("def test_old(): assert True\n")
        base = save(repo, "original base already passes the future reproducer")
        (repo / "product.py").write_text("def value(): return 1\n")
        reviewed = save(repo, "reviewed change has a bug")
        (repo / "product.py").write_text("def value(): return 2\ndef new_name(): return 2\n")
        (repo / "test_value.py").write_text(
            "import pytest, product\n"
            "def test_reproduces(): assert product.value() == 2\n"
            "def test_wrongly_passes(): assert product.value() > 0\n"
            "@pytest.fixture\n"
            "def ready():\n"
            "    if product.value() == 1: raise AssertionError('setup is not evidence')\n"
            "def test_setup(ready): assert product.value() == 2\n"
            "def test_error():\n"
            "    if product.value() == 1: raise RuntimeError('call error')\n"
            "def test_added(): assert product.new_name() == 2\n"
            "@pytest.mark.skip(reason='fixture')\n"
            "def test_skipped(): assert False\n"
            "def uncollected(): assert True\n")
        fixed = save(repo, "fix with only round test edits for the reviewed tree")
        cases = [("test_reproduces", Verdict.PASS), ("test_wrongly_passes", Verdict.FAIL),
                 ("test_setup", Verdict.FAIL), ("test_error", Verdict.FAIL),
                 ("test_added", Verdict.PASS), ("test_skipped", Verdict.FAIL),
                 ("uncollected", Verdict.FAIL), ("test_missing", Verdict.FAIL)]
        plan = tuple(Check(name, kind, (f"test_value.py::{name}",)) for name, _ in cases)
        plan += (Check("missing file", kind, ("test_absent.py::test_absent",)),)
        results = run_checks(repo, base, fixed, plan, reviewed=reviewed, container=container)
        print(result_json(results), flush=True)
        assert [result.verdict for result in results] == [v for _, v in cases] + [Verdict.FAIL]
        assert results[0].tests[0].base == "assertion", "the original base is not the reviewed commit"
        assert results[1].tests[0].base == "passed"
        assert results[2].tests[0].base == "error", "setup assertions cannot reproduce a finding"
        assert results[4].tests[0].base == "added-name"
        verify_action_reviewed(repo, base, reviewed, fixed, container)
        # A reviewed commit need not be reachable from the final head after a rebase.
        git(repo, "checkout", "--quiet", "--detach", base)
        (repo / "unrelated.txt").write_text("another history\n")
        unrelated = save(repo, "unrelated head")
        try:
            run_checks(repo, base, unrelated, plan, container=container)
        except ValueError:
            pass
        else:
            raise AssertionError("a reproducing check requires an exact reviewed commit")
    print("reproducing fixture repos passed; no skips", flush=True)


if __name__ == "__main__":
    verify(Container(sys.argv[1]))
