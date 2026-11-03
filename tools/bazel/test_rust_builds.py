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


class RustDependencyTests(unittest.TestCase):
    """Evaluate the macro and follow its declared edges, including the build script.

    Remote Bazel queries check the fully expanded graph as well.
    The recorder lets this contract run as an ordinary remote Python test.
    """

    def setUp(self):
        self.rules = {}

        def record(**attributes):
            self.rules[":" + attributes["name"]] = attributes

        namespace = {
            "load": lambda *args: None,
            "native": SimpleNamespace(package_name=lambda: "crates/controller",
                                      exports_files=lambda *args: None,
                                      filegroup=record, glob=lambda patterns, **kwargs: [
                                          path for path in patterns if "*" not in path]),
            "all_crate_deps": lambda **kwargs: [],
            "aliases": lambda **kwargs: {},
            "nonhermetic": lambda name: ([], {}),
            "select": lambda choices: choices["//conditions:default"],
            "depset": lambda items: SimpleNamespace(to_list=lambda: list(dict.fromkeys(items))),
            **{name: record for name in ["rust_test", "py_test", "rust_library",
                                        "rust_binary", "cargo_build_script"]},
        }
        sizing = {}
        path = Path(__file__).with_name("test_sizing.bzl")
        exec(compile(path.read_text(), str(path), "exec"), sizing)
        namespace["test_size"] = sizing["test_size"]
        source = Path(__file__).with_name("rust.bzl").read_text()
        exec(re.sub(r"^load\(.*\)$", "", source, flags=re.MULTILINE), namespace)
        namespace["tl3_crate"](
            package="tl3-controller", crate="tl3_controller", build_script=True,
            bins=["tl3", "tl3d"], declared_tests={
                name: {"path": "tests/" + name + ".rs"}
                for name in ["core_defects", "workers", "publishing", "prismabuild",
                             "github", "hindsight", "writer_references", "writer_blocker",
                             "mergify", "simulation"]})

    def dependencies(self, target):
        pending, found = [":" + target], set()
        while pending:
            label = pending.pop()
            if label in found:
                continue
            found.add(label)
            rule = self.rules.get(label, {})
            for attribute in ["srcs", "data", "compile_data", "deps"]:
                pending.extend(rule.get(attribute, []))
        return found

    def test_fake_only_controller_test_does_not_depend_on_harness_or_worker_runtime(self):
        deps = self.dependencies("test_test_core_defects")
        self.assertFalse(sorted(label for label in deps if label.startswith("//harness")), deps)
        self.assertNotIn("//tools/bazel:worker_python", deps)
        self.assertNotIn("//workers:runtime_files", deps)
        self.assertNotIn(":tl3d", deps)
        # Schemas and prompts are compiled into the controller; keep them.
        self.assertIn("//workers:schemas", deps)

    def test_controller_tests_do_not_add_redactor_runtime(self):
        for target in self.rules:
            self.assertNotIn("//tools/bazel:redactor_python", self.dependencies(target[1:]))


    def test_real_worker_scenarios_keep_the_python_environment(self):
        for name in ["workers", "publishing", "github-flow", "writer_references"]:
            with self.subTest(name=name):
                self.assertIn("//tools/bazel:worker_python",
                              self.dependencies("test_test_" + name))

    def test_writer_blocker_keeps_its_compile_time_recording_without_worker_python(self):
        binary = self.rules[":test_test_writer_blocker_binary"]
        self.assertIn("//workers/tests:fixtures", binary["compile_data"])
        self.assertNotIn("//tools/bazel:worker_python",
                         self.dependencies("test_test_writer_blocker"))

    def test_staged_harness_and_role_directory_inputs_are_kept(self):
        for name in ["workers", "publishing", "prismabuild"]:
            with self.subTest(name=name):
                self.assertIn("//harness/src/tl3_harness",
                              self.dependencies("test_test_" + name))
        self.assertIn("//workers/src:role_files", self.dependencies("test_test_hindsight"))

    def test_github_worker_fixtures_keep_their_shared_reply_module(self):
        deps = self.dependencies("test_test_github-flow")
        self.assertIn("tests/worker_model.py", deps)
        self.assertIn("tests/worker_transport.py", deps)
        self.assertIn("tests/worker_replies.py", deps)

    def test_measured_controller_shard_configuration(self):
        for name, count in [("workers", 8), ("publishing", 0),
                            ("mergify", 8), ("github-flow", 50),
                            ("github-notices", 32), ("simulation", 16),
                            ("core_defects", 0)]:
            with self.subTest(name=name):
                self.assertEqual(self.rules[":test_test_" + name].get("shard_count"), count)
        worker = self.rules[":test_test_workers"]
        self.assertEqual(worker["timeout"], "eternal")
        self.assertEqual(worker["exec_properties"], {
            "EstimatedComputeUnits": "1", "EstimatedCPU": "3", "EstimatedMemory": "1GB",
        })

    def test_github_notices_do_not_pull_in_the_flow_group_workers(self):
        self.assertNotIn("//tools/bazel:worker_python",
                         self.dependencies("test_test_github-notices"))


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
            "package": lambda **kwargs: None,
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
        sizing = {}
        path = root / "tools/bazel/test_sizing.bzl"
        exec(compile(path.read_text(), str(path), "exec"), sizing)
        namespace["test_size"] = sizing["test_size"]
        for name in ["nonhermetic.bzl", "python.bzl", "rust.bzl"]:
            path = root / "tools/bazel" / name
            if path.exists():
                source = re.sub(r"^load\(.*\)$", "", path.read_text(), flags=re.MULTILINE)
                exec(compile(source, str(path), "exec"), namespace)
        if package.startswith("crates/"):
            source = re.sub(r"^load\(.*\)$", "", (directory / "BUILD.bazel").read_text(),
                            flags=re.MULTILINE)
            exec(compile(source, str(directory / "BUILD.bazel"), "exec"), namespace)
        else:
            names = (["test_report_limits", "test_harness", "test_future_nested_suite"]
                     if package == "harness/tests" else ["test_sizing", "test_classifier", "test_pb_tests"])
            for name in names:
                namespace["tl3_py_test"](name=name, srcs=[name + ".py"])
        self.postgres_packages = namespace["_POSTGRES"]
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
        self.rules("crates/store")
        for package in self.postgres_packages:
            self.assert_sized(package)

    def test_nested_python_suites_are_sized(self):
        self.assert_sized("harness/tests")

    def test_workers_nested_pytest_is_sized(self):
        rule = self.rules("workers/tests")["test_pb_tests"]
        for key in ["EstimatedComputeUnits", "EstimatedCPU", "EstimatedMemory"]:
            self.assertIn(key, rule["exec_properties"])
            self.assertGreater(float(rule["exec_properties"][key].removesuffix("GB")), 0)

    def test_sizing_loads_without_recorder_globals(self):
        path = Path(__file__).with_name("test_sizing.bzl")
        namespace = {}
        exec(compile(path.read_text(), str(path), "exec"), namespace)
        helper = namespace["test_size"]
        self.assertNotIn("native", helper.__code__.co_names)
        self.assertIn("EstimatedComputeUnits", helper("workers/tests", "test_pb_tests"))

    def test_sizing_preserves_network_and_leaves_other_tests_dynamic(self):
        rules = self.rules("workers/tests")
        self.assertEqual(rules["test_sizing"]["exec_properties"].get("dockerNetwork"), "bridge")
        self.assertNotIn("EstimatedComputeUnits", rules["test_classifier"]["exec_properties"])


if __name__ == '__main__':
    unittest.main()
