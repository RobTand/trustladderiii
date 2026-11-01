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
