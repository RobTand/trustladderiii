"""Contracts that must hold before the shadow gate is introduced."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class WorkspaceTests(unittest.TestCase):
    def test_bzlmod_and_cargo_source_of_truth(self):
        module = (ROOT / 'MODULE.bazel').read_text()
        for required in ['rules_rust', 'crate.from_cargo(', '//:Cargo.toml', '//:Cargo.lock', '1.99.1',
                         'rules_python', 'rules_python_gazelle_plugin', 'pip.parse(']:
            self.assertIn(required, module)
        self.assertTrue((ROOT / '.bazelversion').read_text().strip())

    def test_remote_execution_uses_a_pinned_ubuntu24_image(self):
        config = (ROOT / '.bazelrc').read_text()
        self.assertIn('remote_executor=grpcs://remote.buildbuddy.io', config)
        self.assertIn('remote_download_minimal', config)
        self.assertIn('build:cache-only', config)
        platform = (ROOT / 'tools/bazel/BUILD.bazel').read_text()
        self.assertIn('rbe-ubuntu24-04@sha256:', platform)
        self.assertNotIn('x-buildbuddy-api-key=', config)


if __name__ == '__main__':
    unittest.main()

class RuntimeInputsTests(unittest.TestCase):
    def test_postgres_is_a_checksummed_input_and_not_a_runtime_download(self):
        repository = (ROOT / 'tools/bazel/postgres.bzl').read_text()
        self.assertIn('sha256', repository)
        self.assertIn('17.4.0', (ROOT / 'MODULE.bazel').read_text())
        cache = (ROOT / 'crates/store/tests/support/cache.rs').read_text()
        self.assertIn('TL3_TEST_POSTGRES_DIR', cache)

    def test_worker_runtime_points_at_locked_bazel_dependencies(self):
        runner = (ROOT / 'tools/bazel/rust_runner.py').read_text()
        self.assertIn('TL3_TEST_UV', runner)
        self.assertIn('worker_python', runner)


class AutomaticTestDiscoveryTests(unittest.TestCase):
    def test_rust_integration_tests_are_discovered_by_bazel_not_enumerated(self):
        macro = (ROOT / 'tools/bazel/rust.bzl').read_text()
        self.assertIn('def tl3_integration_tests(', macro)
        self.assertIn('native.glob(["tests/*.rs"])', macro)
        for build in sorted((ROOT / 'crates').glob('*/BUILD.bazel')):
            with self.subTest(file=str(build)):
                text = build.read_text()
                self.assertIn('tl3_integration_tests(', text)
                self.assertNotRegex(text, r'name = "test_test_')
