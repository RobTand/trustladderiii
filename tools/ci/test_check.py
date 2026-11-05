import contextlib
import hashlib
import gzip
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import before_upload
import check


class DiscoveryTests(unittest.TestCase):
    def test_python_lint_requires_command_id_policy(self):
        """State-table Command row: the required Gate checks caller IDs."""
        with patch.object(check, 'run') as run:
            check.python_lint([])
        run.assert_any_call(sys.executable, 'crates/controller/tools/check_command_ids.py')

    def test_shared_checker_matches_the_coordinator_pin(self):
        expected = 'decd9ce671299ece1b245811af459349ed9932ee9b81b2c4c7a13cf96b94f609'
        pin = Path(__file__).with_name('check_no_secret_leak.sha256')
        scanner = pin.with_suffix('.py')
        self.assertTrue(pin.is_file(), 'shared checker hash receipt is missing')
        self.assertEqual(pin.read_text().split()[0], expected)
        self.assertEqual(hashlib.sha256(scanner.read_bytes()).hexdigest(), expected)

    def test_upload_hook_rejects_a_leak_without_printing_its_value(self):
        secret = 'synthetic-fixture-secret-for-upload'
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory, 'gate.log')
            log.write_text(secret)
            output = io.StringIO()
            with patch.dict(os.environ, {'BUILDBUDDY_API_KEY': secret}), \
                 patch.object(sys, 'argv', ['before_upload.py', '--env', 'BUILDBUDDY_API_KEY', directory]), \
                 contextlib.redirect_stdout(output):
                status = before_upload.main()
        self.assertEqual(status, 1)
        self.assertFalse(secret in output.getvalue())
        self.assertIn('gate.log', output.getvalue())

    def test_allowlist_preserves_runtime_settings_and_rejects_unknown_names(self):
        source = {'PATH': '/usr/bin', 'HOME': '/tmp/fixture',
                  'TEST_TMPDIR': '/tmp/test', 'TEST_FIXTURE_TOKEN': 'synthetic-fixture',
                  'RUNFILES_API_KEY': 'synthetic-fixture', 'UNKNOWN_SETTING': 'discard',
                  'RUSTC_BOOTSTRAP': '1', 'BUILDBUDDY_ARTIFACTS_DIRECTORY': '/tmp/artifacts'}
        env = check.child_env(source)
        for name in ['PATH', 'HOME', 'TEST_TMPDIR', 'RUSTC_BOOTSTRAP',
                     'BUILDBUDDY_ARTIFACTS_DIRECTORY']:
            self.assertEqual(env.get(name), source[name])
        for name in ['TEST_FIXTURE_TOKEN', 'RUNFILES_API_KEY', 'UNKNOWN_SETTING']:
            self.assertFalse(name in env, name)

    def test_python_children_do_not_inherit_runner_credentials(self):
        names = ['BUILDBUDDY_API_KEY', 'MERGIFY_TOKEN', 'GITHUB_TOKEN',
                 'OTHER_API_KEY', 'OTHER_TOKEN']
        code = ('import json, os; print(json.dumps([name for name in os.environ '
                'if name.endswith(("_API_KEY", "_TOKEN"))]))')
        with patch.dict(os.environ, {name: 'synthetic-fixture' for name in names}):
            present = json.loads(check.run(sys.executable, '-c', code, capture=True))
        self.assertEqual(present, [])

    def test_workers_use_four_isolated_pytest_processes_and_other_projects_stay_serial(self):
        files = list(map(Path, [
            "workers/pyproject.toml", "workers/uv.lock", "workers/tests/test_workers.py",
            "harness/pyproject.toml", "harness/tests/test_harness.py",
        ]))
        with patch.object(check, "run") as run:
            check.python(files)
        calls = {call.kwargs["cwd"].relative_to(check.ROOT): call.args
                 for call in run.call_args_list if "pytest" in call.args}
        workers = calls[Path("workers")]
        self.assertIn("pytest-xdist==3.8.0", workers)
        self.assertEqual(workers[workers.index("-n") + 1], "4")
        self.assertNotIn("-n", calls[Path("harness")])

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
        with patch.object(check, "run", command), patch.object(check.rust_gate, "test_workspace") as gate:
            check.rust(files)
        tested = [call.args[0] for call in gate.call_args_list]
        self.assertEqual(tested, [Path("Cargo.toml"), Path("standalone/Cargo.toml")])

    def test_failed_lint_stops_the_job(self):
        with patch.object(check, "run", side_effect=subprocess.CalledProcessError(1, "ruff")):
            with self.assertRaises(subprocess.CalledProcessError):
                check.python([Path("tools/queue_pr.py")])


class ScannerCompressionTests(unittest.TestCase):
    SECRET = 'synthetic-scanner-regression-value'

    def scan(self, payload):
        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory, 'execution_log.binpb.zst')
            artifact.write_bytes(payload)
            output = io.StringIO()
            with patch.dict(os.environ, {'SCAN_FIXTURE': self.SECRET}), \
                 contextlib.redirect_stdout(output):
                status = before_upload.main(['--env', 'SCAN_FIXTURE', directory])
        self.assertNotIn(self.SECRET, output.getvalue())
        return status, output.getvalue()

    @staticmethod
    def zstd_frame(payload):
        assert len(payload) < 256
        return (bytes.fromhex('28b52ffd20') + bytes([len(payload)])
                + ((len(payload) << 3) | 1).to_bytes(3, 'little') + payload)

    def test_upload_without_zstd_rejects_unscanned_members(self):
        for payload, expected in ((b'clean', 0), (self.SECRET.encode(), 1),
                                  (self.zstd_frame(b'clean'), 75),
                                  (self.zstd_frame(self.SECRET.encode()), 75)):
            with self.subTest(payload=payload[:4], expected=expected), \
                 patch.dict(sys.modules, {'compression': None, 'compression.zstd': None}):
                status, output = self.scan(payload)
                if expected == 75:
                    self.assertIn('zstd: not scanned (compression.zstd unavailable)', output)
                self.assertEqual(status, expected, output)

    def test_upload_rejects_other_not_scanned_statuses(self):
        with tempfile.TemporaryDirectory() as directory, \
             patch.dict(os.environ, {'SCAN_FIXTURE': ''}), \
             contextlib.redirect_stdout(io.StringIO()) as output:
            Path(directory, 'clean.log').write_text('clean')
            status = before_upload.main(['--env', 'SCAN_FIXTURE', directory])
        self.assertIn('not available, not scanned', output.getvalue())
        self.assertEqual(status, 75)

    def test_real_bazel_execution_log_is_clean(self):
        # Full Bazel 9.3.1 execution log from TL3 checkout t3-b997de7d,
        # captured 2026-11-02. SHA256 is recorded with the fixture.
        artifact = Path(__file__).with_name('fixtures') / 'execution_log.binpb.zst'
        payload = artifact.read_bytes()
        expected = artifact.with_name('execution_log.sha256').read_text().split()[0]
        self.assertEqual(hashlib.sha256(payload).hexdigest(), expected)
        status, output = self.scan(payload)
        self.assertEqual(status, 0, output)
        self.assertNotIn('not scanned', output)

    def test_zstd_log_with_incidental_gzip_magic_is_clean(self):
        record = b'\x0a\x0a/bin/clang\x12\x04\x1f\x8b\x00\xff'
        status, output = self.scan(self.zstd_frame(bytes([len(record)]) + record))
        self.assertEqual(status, 0, output)
        self.assertNotIn('not scanned', output)

    def test_unknown_binary_still_scans_raw_values(self):
        for planted in (False, True):
            with self.subTest(planted=planted):
                payload = b'\x00binary\x1f\x8b\x00\xff\x28\xb5\x2f\xfd\xff'
                payload += self.SECRET.encode() if planted else b'clean'
                status, output = self.scan(payload)
                self.assertEqual(status, int(planted), output)

    def test_corrupt_claimed_gzip_and_concatenated_member_fail_closed(self):
        broken = b'\x1f\x8b\x08broken'
        for payload in (broken, gzip.compress(b'clean') + broken):
            with self.subTest(concatenated=payload != broken):
                status, output = self.scan(payload)
                self.assertEqual(status, 75, output)
                self.assertIn('gzip member unreadable', output)

    def test_corrupt_claimed_zstd_fails_closed(self):
        status, output = self.scan(bytes.fromhex('28b52ffdff'))
        self.assertEqual(status, 75, output)
        self.assertIn('zstd member unreadable', output)

    def test_embedded_and_nested_compressed_values_are_detected(self):
        secret = gzip.compress(self.SECRET.encode())
        for payload in (b'prefix' + secret, gzip.compress(secret),
                        self.zstd_frame(secret),
                        self.zstd_frame(b'clean') + self.zstd_frame(secret)):
            with self.subTest(size=len(payload)):
                status, output = self.scan(payload)
                self.assertEqual(status, 1, output)


if __name__ == "__main__":
    unittest.main()
