"""Crate BUILD files carry Cargo facts; auto-discovered tests stay out of them."""
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).parent))
import rust_builds  # noqa: E402


def crate(root, name, targets, manifest='', dependencies=()):
    directory = root / 'crates' / name
    directory.mkdir(parents=True)
    (directory / 'Cargo.toml').write_text(f'[package]\nname = "{name}"\n' + manifest)
    return {'name': name, 'manifest_path': str(directory / 'Cargo.toml'),
            'dependencies': list(dependencies),
            'targets': [{'kind': [kind], 'name': target, 'src_path': str(directory / source)}
                        for kind, target, source in targets]}


class RustBuildsTests(unittest.TestCase):
    def render(self, root, packages):
        original = rust_builds.ROOT
        rust_builds.ROOT = root
        try:
            return {str(path.relative_to(root)): text
                    for path, text in rust_builds.expected_builds({'packages': packages}).items()}
        finally:
            rust_builds.ROOT = original

    def test_only_tests_cargo_would_not_auto_discover_are_declared(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            builds = self.render(root, [crate(root, 'tl3-demo', [
                ('lib', 'tl3_demo', 'src/lib.rs'),
                ('test', 'plain', 'tests/plain.rs'),
                ('test', 'nested', 'tests/nested/main.rs'),
                ('test', 'scenarios', 'tests/scenarios.rs'),
                ('test', 'renamed', 'checks/renamed.rs'),
                ('bin', 'tool', 'src/bin/tool.rs'),
                ('example', 'outside', '../../deploy/helper.rs'),
                ('custom-build', 'build-script-build', 'build.rs'),
            ], '[[test]]\nname = "scenarios"\nharness = false\n')])
        text = builds['crates/tl3-demo/BUILD.bazel']
        self.assertIn('"scenarios": {"path": "tests/scenarios.rs", "harness": False}', text)
        self.assertIn('"renamed": {"path": "checks/renamed.rs"}', text)
        self.assertNotIn('plain', text)
        self.assertNotIn('nested', text)
        self.assertIn('bins = ["tool"]', text)
        self.assertIn('"outside": "//deploy:helper.rs"', text)
        self.assertIn('build_script = True', text)

    def test_features_are_unified_across_workspace_dependents(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            kernel = crate(root, 'tl3-kernel', [('lib', 'tl3_kernel', 'src/lib.rs')])
            user = crate(root, 'tl3-user', [('lib', 'tl3_user', 'src/lib.rs')], dependencies=[
                {'name': 'tl3-kernel', 'kind': None, 'features': ['serde'],
                 'path': str(root / 'crates/tl3-kernel')}])
            builds = self.render(root, [kernel, user])
        self.assertIn('features = ["serde"]', builds['crates/tl3-kernel/BUILD.bazel'])
        self.assertIn('"//crates/tl3-kernel:tl3_kernel"', builds['crates/tl3-user/BUILD.bazel'])


if __name__ == '__main__':
    unittest.main()
