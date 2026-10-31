"""Python timing splits must respect project ownership."""
import importlib.util
from pathlib import Path
import unittest
import sys

MODULE = Path(__file__).resolve().parents[1] / "circleci.py"
sys.path.insert(0, str(MODULE.parent))
spec = importlib.util.spec_from_file_location("tl3_circleci", MODULE)
ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci)


class Selection(unittest.TestCase):
    def test_nested_python_projects_own_their_tests(self):
        files = list(map(Path, ("workers/tests/test_one.py", "harness/tests/test_two.py", "tools/ci/tests/test_circleci.py", "deploy/ci-runners/test_runners.py", "workers/tests/fixtures/test_broken.py")))
        groups = ci.python_groups(files, {Path("workers"): {}, Path("harness"): {}})
        self.assertEqual(groups, {Path("workers"): [Path("workers/tests/test_one.py")], Path("harness"): [Path("harness/tests/test_two.py")], Path("."): [Path("deploy/ci-runners/test_runners.py"), Path("tools/ci/tests/test_circleci.py")]})


if __name__ == "__main__":
    unittest.main()
