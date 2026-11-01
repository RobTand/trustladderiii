"""Run the trusted action entry point with a hostile inherited Git directory."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[4]


class ActionGitTests(unittest.TestCase):
    def test_action_git_ignores_hostile_environment(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = {**os.environ, "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull}
            env = {key: value for key, value in env.items() if not key.startswith("GIT_")}
            env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull)

            def git(repo, *args):
                return subprocess.check_output(["git", "-C", str(repo), "-c", "user.name=Fixture",
                    "-c", "user.email=fixture@example.invalid", "-c", "core.hooksPath=/dev/null", *args], env=env).decode().strip()

            for name in ("intended", "foreign"):
                repo = root / name
                repo.mkdir()
                git(repo, "init", "-q", "--template=")
                (repo / "value").write_text(name)
                git(repo, "add", ".")
                git(repo, "commit", "-qm", name)
            head = git(root / "intended", "rev-parse", "HEAD")
            staged = root / "input"
            staged.mkdir()
            git(root / "intended", "bundle", "create", str(staged / "served.bundle"), "HEAD")
            shutil.copyfile(ROOT / "crates/controller/src/adapters/prismabuild/action.py", staged / "action.py")
            trusted = staged / "trusted/tl3_harness"
            trusted.mkdir(parents=True)
            shutil.copyfile(ROOT / "harness/src/tl3_harness/trees.py", trusted / "trees.py")
            # The action's Git preparation is real; the container boundary returns
            # committed evidence without launching a nested fleet action.
            (trusted / "__init__.py").write_text('''from .trees import git
Check = lambda *a: a
Container = lambda image, *, exec_scratch=False: (image, exec_scratch)
Kind = lambda x: x
def run_checks(repo, *a, **kw):
    assert git(repo, "show", "HEAD:value") == b"intended"
    assert git(repo, "config", "--get", "protocol.allow").strip() == b"never"
    return []
''')
            (trusted / "execution.py").write_text('import json\nrequire_admission = lambda: None\nresult_json = json.dumps\n')
            (staged / "request.json").write_text(json.dumps({"effect_key": "fixture", "head": head,
                "base": head, "plan": [], "image": "fixture"}))
            env.update(GIT_DIR=str(root / "foreign/.git"), GIT_WORK_TREE=str(root / "foreign"),
                GIT_INDEX_FILE=str(root / "hostile-index"), GIT_CONFIG_COUNT="1",
                GIT_CONFIG_KEY_0="protocol.allow", GIT_CONFIG_VALUE_0="always")
            result = subprocess.run([sys.executable, "-I", "-B", str(staged / "action.py")],
                env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("TL3_CHECKS ", result.stdout)
