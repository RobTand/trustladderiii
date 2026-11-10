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


def stage(root, *, plan=PLAN, conftest="# shared fixtures\n", config=CONFIG, link=False, protected=None, observer=None, footprint=(), generated=()):
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
    (repo / "pyproject.toml").write_text(config)
    (repo / "tests/conftest.py").write_text("# shared fixtures\n")
    (repo / "tests/test_value.py").write_text("def test_value(): assert True\n")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "base")
    base = git(repo, "rev-parse", "HEAD")
    (repo / "tests/conftest.py").write_text(conftest)
    (repo / "README.md").write_text("maker\n")
    if observer is not None:
        (repo / "tests/test_value.py").write_text(observer)
    if link:
        (repo / "secret").symlink_to("/host-sentinel")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "maker")
    head = git(repo, "rev-parse", "HEAD")
    git(repo, "branch", "tl3-base", base)

    staged = root / "input"
    staged.mkdir()
    git(repo, "bundle", "create", str(staged / "served.bundle"), "HEAD", "refs/heads/tl3-base")
    shutil.copyfile(ROOT / "crates/controller/src/checks/action.py", staged / "action.py")
    shutil.copytree(ROOT / "harness/src/tl3_harness", staged / "trusted/tl3_harness",
                    ignore=shutil.ignore_patterns("__pycache__"))
    (staged / "request.json").write_text(json.dumps({
        "effect_key": "effect:89:7", "base": base, "head": head, "reviewed": None,
        "plan": plan, "image": "sha256:" + "1" * 64, "footprint": list(footprint), "generated_paths": list(generated),
        **({} if protected is None else {"protected_paths": protected})}))

    # Supply the Flux launch contract; refusals must return before any container runs.
    output = root / "output"
    output.mkdir()
    home = root / "home"
    home.mkdir(mode=0o700)
    context = root / "context.json"
    context.write_text(json.dumps({
        "owner": "fixture", "correlation": "fixture", "request_digest": "a" * 64,
        "run": {"backend": "flux", "uri": "local:///fixture/flux", "instance": "fixture", "job": "f123"},
        "scratch": {"leaf": str(root / "scratch")},
        "progress": {"journal": str(output / "progress.jsonl"), "no_progress_seconds": 60},
        "output_dir": str(output),
    }))
    cli = root / "backend-cli"
    cli.write_text(f"#!{sys.executable}\n" + """import json, sys
from pathlib import Path
request = json.load(sys.stdin)
assert request == {'op': 'launch_context', 'owner': 'fixture', 'correlation': 'fixture', 'job': 'f123'}
context = json.loads(Path(sys.argv[2]).read_text())
print(json.dumps({'schema': 'tl3.run_backend.frame.v1', 'sequence': 0, 'kind': 'result',
                  'result': {'op': 'launch_context', 'context': context}}))
""")
    cli.chmod(0o700)
    env.update(TL3_EXECUTION_BACKEND="flux", TL3_RUN_OWNER="fixture", TL3_RUN_CORRELATION="fixture",
               FLUX_JOB_ID="f123", TL3_RUN_BACKEND_CLI=str(cli), TL3_RUN_BACKEND_CONFIG=str(context),
               TL3_RUN_BACKEND_SESSION="0", HOME=str(home), XDG_RUNTIME_DIR=str(home), XDG_CACHE_HOME=str(home))
    return staged, env


def lines(output, prefix):
    return [json.loads(line.removeprefix(prefix)) for line in output.splitlines() if line.startswith(prefix)]


def act(staged, env):
    return subprocess.run([sys.executable, "-I", "-B", str(staged / "action.py")],
                          env=env, capture_output=True, text=True)


class ActionRefusalTests(unittest.TestCase):
    def test_checking_supplies_the_maker_footprint_to_real_run_checks(self):
        observer = ("def first(value): return sorted(value)\n"
                    "def second(box): box['x'] = 1\n"
                    "def test_value(): assert True\n")
        plan = [{"name": "proof", "kind": "demonstrating", "tests": [TEST],
                 "proof": {"subjects": [], "observer_support": ["tests/test_value.py::first", "tests/test_value.py::second"],
                           "red_message": "regression"}}]
        for owned, generated in [(False, ()), (True, ()), (True, ("tests/*.py",))]:
            with self.subTest(owned=owned, generated=generated), tempfile.TemporaryDirectory() as directory:
                staged, env = stage(Path(directory), plan=plan, observer=observer,
                                    footprint=("tests/test_value.py",) if owned else (), generated=generated)
                result = act(staged, env)
                self.assertEqual(result.returncode, 0, result.stderr)
                report = lines(result.stdout, "TL3_CHECKS ")[0]
                message = report["results"][0]["tests"][0].get("proof_refusal", report["results"][0]["tests"][0]["message"])
                self.assertIn("proof.support.call_allowlist", message)
                self.assertIn("proof.support.member_write", message)
                if owned and not generated:
                    self.assertIn('Change "tests/test_value.py"', message)
                    self.assertNotIn("cant_meet", message)
                else:
                    self.assertIn("cant_meet", message)
                    self.assertNotIn('Change "', message)

    def test_symlink_in_the_head_is_a_failing_check_naming_it(self):
        # The maker's head is the maker's change: refuse it as a check, not an action error.
        with tempfile.TemporaryDirectory() as directory:
            staged, env = stage(Path(directory), link=True)
            result = act(staged, env)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(lines(result.stdout, "TL3_CHECKS_ERROR "), [])
            reports = lines(result.stdout, "TL3_CHECKS ")
            self.assertEqual([check["verdict"] for check in reports[0]["results"]], ["fail"])
            self.assertIn("secret", reports[0]["results"][0]["tests"][0]["message"])

    def test_repository_protected_path_change_is_reported_not_run(self):
        with tempfile.TemporaryDirectory() as directory:
            staged, env = stage(Path(directory), protected=["README.md"])
            result = act(staged, env)
            self.assertEqual(result.returncode, 0, result.stderr)
            reports = lines(result.stdout, "TL3_CHECKS ")
            self.assertEqual([check["verdict"] for check in reports[0]["results"]], ["fail"])
            self.assertIn("README.md", reports[0]["results"][0]["tests"][0]["message"])

    def test_repository_configuration_error_stays_upstream(self):
        with tempfile.TemporaryDirectory() as directory:
            staged, env = stage(Path(directory), config='[project]\nname = "served"\n')
            result = act(staged, env)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("KeyError", result.stderr)
            self.assertEqual(lines(result.stdout, "TL3_CHECKS_ERROR "), [])

    def test_flux_environment_error_stays_upstream(self):
        with tempfile.TemporaryDirectory() as directory:
            staged, env = stage(Path(directory))
            Path(env["TL3_RUN_BACKEND_CONFIG"]).write_text("not a launch context")
            result = act(staged, env)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("JSONDecodeError", result.stderr)
            self.assertEqual(lines(result.stdout, "TL3_CHECKS_ERROR "), [])

    def test_malformed_check_plan_names_the_harness(self):
        with tempfile.TemporaryDirectory() as directory:
            staged, env = stage(Path(directory), plan=[{"name": "value", "kind": "bogus", "tests": [TEST]}])
            result = act(staged, env)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual([error["error"] for error in lines(result.stdout, "TL3_CHECKS_ERROR ")],
                             ["HarnessInputError"])

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
                {"schema": "tl3.checks.error.v1", "effect_key": "effect:89:7", "error": "HarnessInputError"}])


if __name__ == "__main__":
    unittest.main()
