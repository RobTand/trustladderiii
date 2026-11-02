"""Harness refusals leave the trusted action as a check report or a named TL3 error (#326)."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[4]
CONFIG = '[tool.tl3.harness]\ncommand=["python", "-m", "pytest"]\ntest-files=["tests/test_*.py"]\n'
TEST = "tests/test_value.py::test_value"
PLAN = [{"name": "value", "kind": "regression", "tests": [TEST]}]


def stage(root, *, plan=PLAN, conftest="# shared fixtures\n"):
    """Stage the real action and trusted harness for a head that sets conftest.py."""
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull)

    def git(repo, *args):
        return subprocess.check_output(["git", "-C", str(repo), "-c", "user.name=Fixture",
            "-c", "user.email=fixture@example.invalid", "-c", "core.hooksPath=/dev/null", *args],
            env=env).decode().strip()

    repo = root / "repo"
    (repo / "tests").mkdir(parents=True)
    git(repo, "init", "-q", "--template=", "-b", "main")
    (repo / "pyproject.toml").write_text(CONFIG)
    (repo / "tests/conftest.py").write_text("# shared fixtures\n")
    (repo / "tests/test_value.py").write_text("def test_value(): assert True\n")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "base")
    base = git(repo, "rev-parse", "HEAD")
    (repo / "tests/conftest.py").write_text(conftest)
    (repo / "README.md").write_text("maker\n")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "maker")
    head = git(repo, "rev-parse", "HEAD")
    git(repo, "branch", "tl3-base", base)

    staged = root / "input"
    staged.mkdir()
    git(repo, "bundle", "create", str(staged / "served.bundle"), "HEAD", "refs/heads/tl3-base")
    shutil.copyfile(ROOT / "crates/controller/src/adapters/prismabuild/action.py", staged / "action.py")
    shutil.copytree(ROOT / "harness/src/tl3_harness", staged / "trusted/tl3_harness",
                    ignore=shutil.ignore_patterns("__pycache__"))
    (staged / "request.json").write_text(json.dumps({
        "effect_key": "effect:89:7", "base": base, "head": head, "reviewed": None,
        "plan": plan, "image": "sha256:" + "1" * 64, "exec_scratch": False}))

    # Admission is a PrismaBuild contract; a refusal must return before any container runs.
    shim = root / "bin/docker"
    shim.parent.mkdir()
    shim.write_text("#!/bin/sh\n# Docker CLI shim fixture\nexit 99\n")
    shim.chmod(0o755)
    env.update(PATH=f"{shim.parent}{os.pathsep}{env['PATH']}",
               PRISMABUILD_CONTAINER_OWNER="fixture", PRISMABUILD_CONTAINER_MARKER=str(root / "marker"),
               PRISMABUILD_ACTION_PROGRESS_PATH=str(root / "progress.json"),
               PRISMABUILD_ACTION_PROGRESS_TOKEN="fixture",
               PRISMABUILD_ACTION_PROGRESS_ALLOWANCES=json.dumps({"tests": 60}))
    return staged, env


def lines(output, prefix):
    return [json.loads(line.removeprefix(prefix)) for line in output.splitlines() if line.startswith(prefix)]


class ActionRefusalTests(unittest.TestCase):
    def test_changed_test_configuration_is_reported_not_raised(self):
        with tempfile.TemporaryDirectory() as directory:
            staged, env = stage(Path(directory),
                                conftest="def pytest_collection_modifyitems(items): items.clear()\n")
            result = subprocess.run([sys.executable, "-I", "-B", str(staged / "action.py")],
                                    env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            reports = lines(result.stdout, "TL3_CHECKS ")
            self.assertEqual(len(reports), 1, result.stdout)
            self.assertEqual([check["verdict"] for check in reports[0]["results"]], ["fail"])
            self.assertIn("tests/conftest.py", reports[0]["results"][0]["tests"][0]["message"])

    def test_rejected_check_input_names_the_harness(self):
        with tempfile.TemporaryDirectory() as directory:
            staged, env = stage(Path(directory), plan=PLAN + PLAN)
            result = subprocess.run([sys.executable, "-I", "-B", str(staged / "action.py")],
                                    env=env, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(lines(result.stdout, "TL3_CHECKS_ERROR "), [
                {"schema": "tl3.checks.error.v1", "effect_key": "effect:89:7", "error": "ValueError"}])


if __name__ == "__main__":
    unittest.main()
