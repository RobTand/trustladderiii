"""Selection must fail open to the full gate when dependencies are unknown."""
import importlib.util
from pathlib import Path
import unittest

MODULE = Path(__file__).resolve().parents[1] / "circleci.py"
spec = importlib.util.spec_from_file_location("tl3_circleci", MODULE)
ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci)


class Selection(unittest.TestCase):
    packages = {"tl3-kernel": Path("crates/kernel"), "tl3-controller": Path("crates/controller")}

    def test_crate_changes_include_reverse_dependencies(self):
        self.assertEqual(ci.rust_filter(["crates/kernel/src/lib.rs"], self.packages), "rdeps(=tl3-kernel)")

    def test_multiple_crates_are_a_union(self):
        self.assertEqual(ci.rust_filter(["crates/kernel/src/lib.rs", "crates/controller/tests/workers.rs"], self.packages), "rdeps(=tl3-controller) | rdeps(=tl3-kernel)")

    def test_queue_and_main_always_run_full_gate(self):
        self.assertEqual(ci.rust_filter(["crates/kernel/src/lib.rs"], self.packages, full=True), "all()")

    def test_unknown_deleted_or_shared_inputs_run_everything(self):
        for path in ("Cargo.lock", "workers/src/worker.py", "crates/deleted/src/lib.rs", ".circleci/config.yml", "tools/ci/check.py"):
            with self.subTest(path=path):
                self.assertEqual(ci.rust_filter([path], self.packages), "all()")
        self.assertEqual(ci.rust_filter([], self.packages), "all()")

    def test_nested_python_projects_own_their_tests(self):
        files = list(map(Path, ("workers/tests/test_one.py", "harness/tests/test_two.py", "tools/ci/tests/test_circleci.py", "deploy/ci-runners/test_runners.py", "workers/tests/fixtures/test_broken.py")))
        groups = ci.python_groups(files, {Path("workers"): {}, Path("harness"): {}})
        self.assertEqual(groups, {Path("workers"): [Path("workers/tests/test_one.py")], Path("harness"): [Path("harness/tests/test_two.py")], Path("."): [Path("deploy/ci-runners/test_runners.py"), Path("tools/ci/tests/test_circleci.py")]})


if __name__ == "__main__":
    unittest.main()
