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
            "services/api/Cargo.toml", "services/spikes/old/Cargo.toml",
            "crates/new/tests/fixtures/example/Cargo.toml", "spikes/old/Cargo.toml",
        ]))
        self.assertEqual(check.rust_manifests(files), list(map(Path, [
            "Cargo.toml", "crates/github/Cargo.toml", "crates/new/nested/Cargo.toml",
            "services/api/Cargo.toml",
        ])))

    def test_python_finds_packages_and_standalone_tests_without_collecting_fixtures(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifests = {
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

    def test_python_projects_use_the_nearest_ancestor(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifests = [
                "workers/nested/pyproject.toml", "pyproject.toml", "workers/pyproject.toml",
            ]
            for name in manifests:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('[project]\nname = "example"\n')
            files = list(map(Path, manifests + [
                "tests/test_root.py", "workers/tests/test_worker.py",
                "workers/nested/tests/test_nested.py",
            ]))
            with patch.object(check, "ROOT", root), patch.object(check, "run") as run:
                check.python(files)
            pytest_calls = {
                call.kwargs["cwd"].relative_to(root): call.args
                for call in run.call_args_list if "pytest" in call.args
            }
            self.assertEqual(set(pytest_calls), {
                Path("."), Path("workers"), Path("workers/nested"),
            })
            self.assertIn("--ignore=workers", pytest_calls[Path(".")])
            self.assertIn("--ignore=nested", pytest_calls[Path("workers")])
            self.assertFalse(any(
                str(arg).startswith("--ignore=")
                for arg in pytest_calls[Path("workers/nested")]
            ))
            self.assertFalse(any("unittest" in call.args for call in run.call_args_list))

    def test_python_project_without_tests_checks_its_lock_instead_of_pytest(self):
        # A deployment environment pins its own interpreter and has no tests to run.
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifests = {
                "workers/pyproject.toml": '[project]\nname = "workers"\n',
                "deploy/service/pyproject.toml":
                    '[project]\nname = "service"\nrequires-python = ">=3.12,<3.13"\n',
                "deploy/service/uv.lock": "version = 1\n",
            }
            for name, content in manifests.items():
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content)
            files = list(map(Path, manifests)) + [Path("workers/tests/test_worker.py")]
            with patch.object(check, "ROOT", root), patch.object(check, "run") as run:
                check.python(files)
            by_directory = {}
            for call in run.call_args_list:
                directory = call.kwargs.get("cwd", root).relative_to(root)
                by_directory.setdefault(directory, []).append(call.args)
            self.assertIn("pytest", by_directory[Path("workers")][0])
            self.assertEqual(by_directory[Path("deploy/service")],
                             [("uv", "lock", "--check", "--project", ".")])

    def test_python_rejects_manifest_without_project_before_running_checks(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = Path("config/pyproject.toml")
            (root / manifest).parent.mkdir()
            (root / manifest).write_text("[tool.ruff]\n")
            with patch.object(check, "ROOT", root), patch.object(check, "run") as run:
                with self.assertRaisesRegex(
                    RuntimeError, r"config/pyproject\.toml.*\[project\]",
                ):
                    check.python([manifest, Path("config/test_config.py")])
            run.assert_not_called()

    def test_workspace_members_run_once_and_standalone_crates_still_run(self):
        calls = []

        def command(*args, **kwargs):
            calls.append(args)
            if args[:2] == ("cargo", "metadata"):
                workspace = "standalone" if "services/standalone/Cargo.toml" in map(str, args) else "."
                return json.dumps({"workspace_root": str(check.ROOT / workspace)})

        files = list(map(Path, [
            "Cargo.toml", "crates/member/Cargo.toml", "services/standalone/Cargo.toml",
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
