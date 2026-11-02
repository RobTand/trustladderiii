"""Crate BUILD files carry Cargo facts; auto-discovered tests stay out of them."""
from pathlib import Path
import sys
import re
from types import SimpleNamespace
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


class TestSizingTests(unittest.TestCase):
    """Expand the real BUILD macros and inspect the test runner rules they emit."""

    def rules(self, package):
        root = Path(__file__).resolve().parents[2]
        directory = root / package
        rules = {}

        def record(**attributes):
            rules[attributes["name"]] = attributes

        def glob(patterns, **kwargs):
            return sorted({str(path.relative_to(directory)) for pattern in patterns
                           for path in directory.glob(pattern) if path.is_file()})

        namespace = {
            "native": SimpleNamespace(package_name=lambda: package, glob=glob,
                                      exports_files=lambda *args: None, filegroup=record,
                                      package_relative_label=lambda label: label),
            "struct": SimpleNamespace,
            "all_crate_deps": lambda **kwargs: [],
            "aliases": lambda **kwargs: {},
            "select": lambda choices: choices["//conditions:default"],
            "depset": lambda items: SimpleNamespace(to_list=lambda: list(dict.fromkeys(items))),
            "requirement": lambda name: name,
            **{name: record for name in ["rust_test", "py_test", "rust_library",
                                        "rust_binary", "cargo_build_script", "_py_library"]},
        }
        for name in ["test_sizing.bzl", "nonhermetic.bzl", "python.bzl", "rust.bzl"]:
            path = root / "tools/bazel" / name
            if path.exists():
                source = re.sub(r"^load\(.*\)$", "", path.read_text(), flags=re.MULTILINE)
                exec(compile(source, str(path), "exec"), namespace)
        if package.startswith("crates/"):
            source = re.sub(r"^load\(.*\)$", "", (directory / "BUILD.bazel").read_text(),
                            flags=re.MULTILINE)
            exec(compile(source, str(directory / "BUILD.bazel"), "exec"), namespace)
        else:
            for path in directory.glob("test_*.py"):
                namespace["tl3_py_test"](name=path.stem, srcs=[path.name])
        return {name: attrs for name, attrs in rules.items()
                if attrs.get("main") in ["//tools/bazel:rust_runner.py",
                                         "//tools/bazel:python_runner.py"]}

    def assert_sized(self, package):
        rules = self.rules(package)
        self.assertTrue(rules, package)
        for name, rule in rules.items():
            with self.subTest(target=f"//{package}:{name}"):
                properties = rule.get("exec_properties", {})
                for key in ["EstimatedComputeUnits", "EstimatedCPU", "EstimatedMemory"]:
                    self.assertIn(key, properties)
                    self.assertGreater(float(properties[key].removesuffix("GB")), 0)

    def test_every_controller_runner_including_worker_shards_is_sized(self):
        self.assert_sized("crates/controller")

    def test_every_postgres_runner_is_sized(self):
        self.assert_sized("crates/store")

    def test_nested_python_suites_are_sized(self):
        self.assert_sized("harness/tests")

    def test_sizing_preserves_network_and_leaves_other_tests_dynamic(self):
        rules = self.rules("workers/tests")
        self.assertEqual(rules["test_planner"]["exec_properties"].get("dockerNetwork"), "bridge")
        self.assertNotIn("EstimatedComputeUnits", rules["test_classifier"]["exec_properties"])


if __name__ == '__main__':
    unittest.main()
