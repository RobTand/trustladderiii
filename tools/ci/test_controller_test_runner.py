"""#791: controller test children receive an executable reply validator before scenarios."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


RUNNER = Path(__file__).with_name('controller_test_runner.sh')


class ControllerTestRunnerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.binary = self.root / 'debug/deps/publishing-fixture'
        self.binary.parent.mkdir(parents=True)
        self.binary.write_text('#!/bin/sh\nprintf "%s\\n" "${TL3_BINARY:-unset}" "$@"\n')
        self.binary.chmod(0o755)

    def run_child(self, *, cargo=False, environment=None):
        env = {key: value for key, value in os.environ.items()
               if key not in {'TL3_BINARY', 'CARGO_PKG_NAME'}}
        env.update(environment or {})
        flags = [] if cargo else ['--controller']
        if cargo:
            env['CARGO_PKG_NAME'] = 'tl3-controller'
        return subprocess.run(['bash', str(RUNNER), *flags, str(self.binary), '--exact', 'scenario'],
                              env=env, text=True, capture_output=True, check=False)

    def validator(self, path):
        path.write_text('#!/bin/sh\nexit 0\n')
        path.chmod(0o755)
        return path

    def test_bazel_supplied_binary_reaches_child_with_arguments(self):
        cli = self.validator(self.root / 'tl3')
        result = self.run_child(environment={'TL3_BINARY': str(cli)})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines(), [str(cli), '--exact', 'scenario'])

    def test_cargo_finds_binary_beside_deps_in_custom_target_directory(self):
        cli = self.validator(self.binary.parent.parent / 'tl3')
        result = self.run_child(cargo=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines()[0], str(cli))

    def test_missing_binary_fails_before_child_runs(self):
        missing = self.root / 'missing-tl3'
        result = self.run_child(environment={'TL3_BINARY': str(missing)})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(f"TL3_BINARY '{missing}' is missing or not executable", result.stderr)
        self.assertEqual(result.stdout, '')

    def test_nonexecutable_binary_fails_before_child_runs(self):
        cli = self.root / 'tl3'
        cli.write_text('not an executable')
        result = self.run_child(environment={'TL3_BINARY': str(cli)})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('missing or not executable', result.stderr)
        self.assertEqual(result.stdout, '')

    def test_cargo_missing_binary_has_setup_diagnostic(self):
        result = self.run_child(cargo=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('build and supply the tl3 check-reply binary', result.stderr)
        self.assertEqual(result.stdout, '')

    def test_other_cargo_packages_do_not_require_controller_binary(self):
        env = {'PATH': os.environ['PATH'], 'CARGO_PKG_NAME': 'tl3-store'}
        result = subprocess.run(['bash', str(RUNNER), str(self.binary)], env=env,
                                text=True, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), 'unset')

    def test_cargo_run_does_not_change_production_binary_environment(self):
        self.binary = self.binary.parent.parent / 'tl3'
        self.binary.write_text('#!/bin/sh\nprintf "%s\\n" "${TL3_BINARY:-unset}"\n')
        self.binary.chmod(0o755)
        result = self.run_child(cargo=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), 'unset')
