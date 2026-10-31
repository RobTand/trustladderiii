"""CircleCI must preserve the shared gate's failures and target policy."""
from contextlib import ExitStack
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

MODULE = Path(__file__).resolve().parents[1] / "circleci.py"
sys.path.insert(0, str(MODULE.parent))
spec = importlib.util.spec_from_file_location("circleci_gate", MODULE)
ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci)
import rust_gate


class SharedGate(unittest.TestCase):
    def setUp(self):
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        self.temporary = Path(self.stack.enter_context(tempfile.TemporaryDirectory()))
        source = self.temporary / "source"
        source.mkdir()
        self.source = source
        self.git("init", "--quiet", "-b", "main", cwd=source)
        (source / "Cargo.toml").write_text('[package]\nname = "fixture"\nversion = "0.1.0"\n')
        policy = source / rust_gate.POLICY
        policy.parent.mkdir(parents=True)
        policy.write_text(json.dumps({"seconds": 1, "slow": {}}))
        self.git("add", ".", cwd=source)
        self.git("commit", "--quiet", "-m", "Target budget", cwd=source)
        self.git("checkout", "--quiet", "-b", "feature", cwd=source)
        self.git("commit", "--quiet", "--allow-empty", "-m", "Feature", cwd=source)
        self.root = self.temporary / "checkout"
        self.git("clone", "--quiet", "--depth=1", "--branch=feature", source.as_uri(), str(self.root), cwd=source)
        self.binary = self.temporary / "binary.py"
        self.binary.write_text(
            'print(\'{"type":"test","event":"ok","name":"one","exec_time":0.25}\')\n'
            'print(\'{"type":"suite","event":"ok","passed":1,"failed":0,"ignored":0}\')\n'
        )
        self.stack.enter_context(patch.dict(os.environ, {
            "PATH": os.environ["PATH"], "RUST_TEST_THREADS": "1",
            "CIRCLE_NODE_INDEX": "0", "CIRCLE_NODE_TOTAL": "1",
        }, clear=True))
        for module in (ci, ci.check, rust_gate):
            self.stack.enter_context(patch.object(module, "ROOT", self.root))
        self.stack.enter_context(patch.object(ci, "RESULTS", self.root / "test-results"))
        self.stack.enter_context(patch.object(ci, "split", return_value=["fixture"]))
        self.stack.enter_context(patch.object(rust_gate, "collect_binaries", return_value=[{
            "name": "fixture/unit", "command": [sys.executable, str(self.binary)], "custom": False,
        }]))
        original_run = ci.check.run

        def discovery(*args, **kwargs):
            if args[:2] == ("cargo", "metadata"):
                return json.dumps({"workspace_root": str(self.root), "packages": [{
                    "name": "fixture", "id": "fixture", "manifest_path": str(self.root / "Cargo.toml"),
                }], "workspace_members": ["fixture"], "target_directory": str(self.root / "target")})
            if args[:2] in (("cargo", "fmt"), ("cargo", "clippy")):
                return ""
            if args[:3] == ("cargo", "nextest", "list"):
                return '{"rust-suites":{"fixture":{}}}'
            if args[:3] == ("cargo", "nextest", "run"):
                return ""
            return original_run(*args, **kwargs)

        self.stack.enter_context(patch.object(ci.check, "run", side_effect=discovery))

    def git(self, *args, cwd=None):
        return subprocess.check_output([
            "git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
            "-c", "commit.gpgsign=false", *args,
        ], cwd=cwd or self.root, text=True).strip()

    def run_circleci(self):
        with patch.object(sys, "argv", ["circleci.py", "rust"]):
            ci.main()

    def test_per_test_budget_failure_reaches_circleci(self):
        self.binary.write_text(self.binary.read_text().replace('"exec_time":0.25', '"exec_time":2'))
        with self.assertRaisesRegex(RuntimeError, "Per-test time budget exceeded"):
            self.run_circleci()

    def test_skipped_tests_fail_circleci(self):
        self.binary.write_text(
            'print(\'{"type":"test","event":"ignored","name":"one"}\')\n'
            'print(\'{"type":"suite","event":"ok","passed":0,"ignored":1}\')\n'
        )
        with self.assertRaisesRegex(RuntimeError, "skipped"):
            self.run_circleci()

    def test_feature_cannot_expand_target_branch_budget(self):
        (self.root / rust_gate.POLICY).write_text(json.dumps({"seconds": 2, "slow": {}}))
        with self.assertRaisesRegex(RuntimeError, "may not grow"):
            self.run_circleci()

    def test_shallow_checkout_recovers_history_and_target_branch(self):
        self.assertEqual(self.git("rev-parse", "--is-shallow-repository"), "true")
        self.run_circleci()
        self.assertEqual(self.git("rev-parse", "--is-shallow-repository"), "false")
        self.assertEqual(self.git("rev-parse", "origin/main"), self.git("rev-parse", "main", cwd=self.source))

    def test_shared_gate_timings_are_uploaded_as_junit(self):
        self.run_circleci()
        reports = list((self.root / "test-results").glob("rust*.xml"))
        self.assertTrue(reports, "shared gate timings were not exported as JUnit")
        cases = [case for report in reports for case in ET.parse(report).iter("testcase")]
        self.assertTrue(any(case.get("classname") == "fixture/unit" and case.get("name") == "one"
                            and float(case.get("time")) == 0.25 for case in cases))


if __name__ == "__main__":
    unittest.main()
