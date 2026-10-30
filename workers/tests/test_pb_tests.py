import json

import pytest

from tl3_harness.test_tool import SUMMARY_BYTES, Summary, infrastructure
from tl3_workers.pb_tests import TestSettings, make_test_tool
from tl3_workers.tools import Worktree, build_tools


IMAGE = "sha256:" + "1" * 64
KEY = "a" * 64


@pytest.fixture
def submission(tmp_path):
    root = tmp_path / "worktree"
    root.mkdir()
    (root / "uncommitted.py").write_bytes(b"current dirty bytes\x00\xff")
    (root / ".gitignore").write_text("uncommitted.py\n")
    scripts = tmp_path / "tools"
    scripts.mkdir()
    (scripts / "pbrun.py").write_text('''
import json
from pathlib import Path
import sys
here = Path(__file__).parent
args = sys.argv[1:]
root = Path(args[args.index("--cwd") + 1])
assert (root / "checkout/uncommitted.py").read_bytes() == b"current dirty bytes\\x00\\xff"
assert (root / "trusted/tl3_harness/runner.py").is_file()
assert (root / "action.py").is_file()
assert not (root / "checkout/.git").exists()
(here / "submitted.json").write_text(json.dumps(args))
print(json.dumps({"schema": "prismabuild.submission.v1", "action_key": "a" * 64}))
''')
    (scripts / "pbwait.py").write_text('''
import hashlib
import json
from pathlib import Path
import sys
here = Path(__file__).parent
(here / "waited.json").write_text(json.dumps(sys.argv[1:]))
result = json.loads((here / "response.json").read_text())
log = here / "action.log"
raw = ("TL3_TEST_SUMMARY " + json.dumps(result) + "\\n").encode()
log.write_bytes(raw)
ending = {"action_key": "a" * 64, "status": "executed", "succeeded": True, "exit_code": 0,
          "result": {"verified": True, "receipt_sha256": "b" * 64,
                     "stdout": {"path": str(log), "bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}}}
if (here / "override.json").exists():
    ending.update(json.loads((here / "override.json").read_text()))
print(json.dumps({"schema": "prismabuild.endings.v1", "complete": True, "endings": [ending]}))
''')
    return root, scripts


def call(submission, response, tests=None, override=None):
    root, scripts = submission
    (scripts / "response.json").write_text(json.dumps(response))
    if override is not None:
        (scripts / "override.json").write_text(json.dumps(override))
    with Worktree(root) as tree:
        tree.test_settings = TestSettings(IMAGE, scripts)
        tool = build_tools(tree, {"run_tests": "Run worktree tests through PrismaBuild."})[0]
        assert set(tool.args) == {"tests"}
        return tool.invoke({} if tests is None else {"tests": tests})


@pytest.mark.parametrize("outcome", ["passed", "failed", "error", "skipped"])
def test_fake_submission_returns_outcomes_and_uses_only_test_action(submission, outcome):
    summary = Summary()
    summary.add("tests/test_code.py::test_value", outcome, "fixture failure" if outcome == "failed" else "")
    result = call(submission, summary.result, ["tests/test_code.py::test_value"])
    assert result == {**summary.result, "action_key": KEY}
    args = json.loads((submission[1] / "submitted.json").read_text())
    assert "--detach" in args and "--anywhere" in args
    assert "--timeout-s" not in args
    assert args[args.index("--container-image") + 1] == IMAGE
    assert args[args.index("--progress-phase") + 1] == "tests=120"
    assert args[args.index("--cpus") + 1] == "1"
    assert args[args.index("--demand") + 1] == "mem_gb=2"
    assert args[args.index("--") + 1:] == ["python3", "-I", "-B", "action.py", IMAGE,
                                         '["tests/test_code.py::test_value"]']
    assert json.loads((submission[1] / "waited.json").read_text()) == ["--endings", "--until", "cleaned", KEY]


def test_whole_suite(submission):
    assert call(submission, Summary().result)["status"] == "completed"
    args = json.loads((submission[1] / "submitted.json").read_text())
    assert args[-1] == "[]"


@pytest.mark.parametrize("override", [
    {"status": "failed", "succeeded": False, "exit_code": 137},
    {"action_key": "c" * 64},
    {"result": {"verified": False}},
    {"result": {"verified": True, "receipt_sha256": None}},
])
def test_fake_infrastructure_failure_is_never_a_test_failure(submission, override):
    result = call(submission, Summary().result, override=override)
    assert result["status"] == "infrastructure_error"
    assert result["tests"] == [] and sum(result["totals"].values()) == 0
    assert result["action_key"] == KEY


def test_reported_container_failure_is_infrastructure(submission):
    result = call(submission, infrastructure("Container failed."))
    assert result["status"] == "infrastructure_error"
    assert result["message"] == "Container failed."


def test_submission_failure(submission):
    (submission[1] / "pbrun.py").write_text('raise SystemExit(1)\n')
    assert call(submission, Summary().result)["status"] == "infrastructure_error"
    assert not (submission[1] / "waited.json").exists()


def test_fixed_summary_cap_with_escaped_unicode_and_many_tests(submission):
    summary = Summary()
    for i in range(10000):
        summary.add(str(i) + "😀\"\\" * 1000, "failed", "😀\"\\" * 10000)
    result = call(submission, summary.result)
    assert result["status"] == "completed"
    assert result["totals"]["failed"] == 10000
    assert len(json.dumps(result).encode()) <= SUMMARY_BYTES
    assert "Truncated" in result["note"]


@pytest.mark.parametrize("tests", [["/etc/passwd"], ["../test.py"], ["-c"], [".git/test.py"],
                                    ["tests/../../test.py"], ["test.py\x00"], ["C:\\test.py"]])
def test_selectors_reject_options_and_escape_without_submission(submission, tests):
    with Worktree(submission[0]) as tree:
        tool = make_test_tool(tree, TestSettings(IMAGE, submission[1]))
        with pytest.raises(ValueError):
            tool(tests)
    assert not (submission[1] / "submitted.json").exists()


def test_missing_settings_is_explicit(submission):
    with Worktree(submission[0]) as tree:
        result = make_test_tool(tree)()
    assert result["status"] == "infrastructure_error"
    assert "settings" in result["message"]


def test_harness_inside_role_worktree_is_refused(submission, monkeypatch):
    from tl3_harness import execution

    monkeypatch.setattr(execution, "__file__", str(submission[0] / "untrusted/execution.py"))
    (submission[0] / "untrusted").mkdir()
    result = call(submission, Summary().result)
    assert result["status"] == "infrastructure_error"
    assert not (submission[1] / "submitted.json").exists()


@pytest.mark.parametrize("response", ["[]", "null", "not json"])
@pytest.mark.parametrize("script", ["pbrun.py", "pbwait.py"])
def test_malformed_prismabuild_response_is_infrastructure(submission, response, script):
    (submission[1] / script).write_text("print(" + repr(response) + ")\n")
    result = call(submission, Summary().result)
    assert result["status"] == "infrastructure_error"
    assert sum(result["totals"].values()) == 0
