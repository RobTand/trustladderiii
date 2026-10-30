import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import check


class DiscoveryTests(unittest.TestCase):
    def test_rust_finds_new_crates_and_root_but_ignores_spikes_and_fixtures(self):
        files = list(map(Path, [
            "Cargo.toml", "crates/github/Cargo.toml", "crates/new/nested/Cargo.toml",
            "crates/new/tests/fixtures/example/Cargo.toml", "spikes/old/Cargo.toml",
        ]))
        self.assertEqual(check.rust_manifests(files), list(map(Path, [
            "Cargo.toml", "crates/github/Cargo.toml", "crates/new/nested/Cargo.toml",
        ])))

    def test_python_finds_packages_and_standalone_tests_without_collecting_fixtures(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifests = {
                "pyproject.toml": "[tool.ruff]\n",
                "workers/pyproject.toml": '[project]\nname = "workers"\n',
                "new/package/pyproject.toml": '[project]\nname = "new"\n',
                "harness/tests/fixtures/pyproject.toml": '[project]\nname = "fixture"\n',
                "spikes/old/pyproject.toml": '[project]\nname = "spike"\n',
            }
            for name, content in manifests.items():
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content)
            files = list(map(Path, manifests)) + list(map(Path, [
                "workers/tests/test_worker.py", "new/package/tests/test_new.py",
                "deploy/github-app/test_setup.py", "tools/ci/test_check.py",
                "harness/tests/fixtures/test_broken.py", "spikes/old/test_old.py",
            ]))
            with patch.object(check, "ROOT", root):
                projects = check.python_projects(files)
            self.assertEqual(set(projects), {Path("workers"), Path("new/package")})
            self.assertEqual(check.standalone_test_dirs(files, projects), [
                Path("deploy/github-app"), Path("tools/ci"),
            ])

    def test_workspace_members_run_once_and_standalone_crates_still_run(self):
        calls = []

        def command(*args, **kwargs):
            calls.append(args)
            if args[:2] == ("cargo", "metadata"):
                workspace = "standalone" if "crates/standalone/Cargo.toml" in map(str, args) else "."
                return json.dumps({"workspace_root": str(check.ROOT / workspace)})

        files = list(map(Path, [
            "Cargo.toml", "crates/member/Cargo.toml", "crates/standalone/Cargo.toml",
        ]))
        with patch.object(check, "run", command):
            check.rust(files)
        tested = [args[3] for args in calls if args[:2] == ("cargo", "test")]
        self.assertEqual(tested, [Path("Cargo.toml"), Path("standalone/Cargo.toml")])

    def test_failed_lint_stops_the_job(self):
        with patch.object(check, "run", side_effect=subprocess.CalledProcessError(1, "ruff")):
            with self.assertRaises(subprocess.CalledProcessError):
                check.python([Path("tools/board.py")])


if __name__ == "__main__":
    unittest.main()
