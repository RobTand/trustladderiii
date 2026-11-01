import json
import os
import stat
import subprocess

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
        assert set(tool.args) == {"tests", "base"}
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


@pytest.mark.parametrize("schema", ["prismabuild.submission.v1", "prismaquant.prismabuild.pbrun_detach.v1"])
def test_published_submission_schemas(submission, schema):
    script = submission[1] / "pbrun.py"
    script.write_text(script.read_text().replace("prismabuild.submission.v1", schema))
    assert call(submission, Summary().result)["status"] == "completed"


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


def test_wait_is_declared_before_blocking_and_on_reattach(submission, monkeypatch):
    from tl3_workers import pb_tests

    root, scripts = submission
    (scripts / "response.json").write_text(json.dumps(Summary().result))
    original = pb_tests._json_command
    events = []
    waits = 0

    def command(args):
        nonlocal waits
        if args[1].endswith("pbwait.py"):
            waits += 1
            assert events[-1] == {"event": "progress", "unit": "external_wait",
                                  "wait": {"upstream": "prismabuild", "key": KEY}}
            if waits == 1:
                return 124, {"schema": "prismabuild.endings.v1", "complete": False,
                             "endings": [{"action_key": KEY, "status": "waiting",
                                          "progress_observation": {"accepted_count": 7}}]}
        return original(args)

    monkeypatch.setattr(pb_tests, "_json_command", command)
    with Worktree(root) as tree:
        tree.progress_emit = events.append
        result = make_test_tool(tree, TestSettings(IMAGE, scripts))()
    assert result["status"] == "completed"
    assert waits == 2
    assert len(events) == 2
    assert events[0] == events[1]


def test_maker_staged_submission_retains_checkout_parent(submission):
    root, scripts = submission

    def git(*args):
        return subprocess.check_output([
            "git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false",
            "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", *args,
        ], cwd=root, text=True).strip()

    git("init", "--quiet", "--template=")
    git("add", ".gitignore")
    git("commit", "--quiet", "-m", "Maker base")
    parent = git("rev-parse", "HEAD")
    before = git("status", "--porcelain")
    script = scripts / "pbrun.py"
    script.write_text(script.read_text().replace(
        '(here / "submitted.json").write_text(json.dumps(args))',
        'import subprocess\n'
        'parent = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()\n'
        '(here / "parent").write_text(parent)\n'
        'shallow = subprocess.check_output(["git", "rev-parse", "--is-shallow-repository"], cwd=root, text=True).strip()\n'
        '(here / "shallow").write_text(shallow)\n'
        '(here / "submitted.json").write_text(json.dumps(args))',
    ))
    (scripts / "response.json").write_text(json.dumps(Summary().result))
    with Worktree(root) as tree:
        tree.test_checkout = root
        result = make_test_tool(tree, TestSettings(IMAGE, scripts))()
    assert result["status"] == "completed"
    assert (scripts / "parent").read_text() == parent, "the action must belong to the maker checkout"
    assert (scripts / "shallow").read_text() == "false", "PrismaBuild requires complete snapshot history"
    assert git("rev-parse", "HEAD") == parent
    assert git("status", "--porcelain") == before
    assert (root / "uncommitted.py").read_bytes() == b"current dirty bytes\x00\xff"


def test_staging_git_uses_closed_protocol_policy(submission, monkeypatch, tmp_path):
    from tl3_workers import pb_tests
    original = subprocess.run
    policies = []

    def run(command, **kwargs):
        if command[0] == "git":
            # Ask the real Git process for its effective policy with the same
            # global arguments, cwd and environment as the staging operation.
            operation = next(i for i, arg in enumerate(command) if arg in ("init", "fetch", "add", "commit", "update-ref"))
            probe = original([*command[:operation], "config", "--get", "protocol.allow"],
                cwd=kwargs["cwd"], env=kwargs["env"], capture_output=True)
            policies.append(probe.stdout.strip())
        return original(command, **kwargs)

    monkeypatch.setenv("GIT_CONFIG_COUNT", "1")
    monkeypatch.setenv("GIT_CONFIG_KEY_0", "protocol.allow")
    monkeypatch.setenv("GIT_CONFIG_VALUE_0", "always")
    monkeypatch.setattr(pb_tests.subprocess, "run", run)
    staged = tmp_path / "input"
    staged.mkdir()
    with Worktree(submission[0]) as tree:
        pb_tests._stage(tree, staged)
    assert policies and all(policy == b"never" for policy in policies), policies


@pytest.mark.parametrize('enabled', [False, True])
def test_scratch_setting_reaches_the_trusted_action(submission, enabled):
    from dataclasses import fields
    assert 'exec_scratch' in {field.name for field in fields(TestSettings)}, 'maker settings must carry the opt-in'
    root, scripts = submission
    (scripts / 'response.json').write_text(json.dumps(Summary().result))
    with Worktree(root) as tree:
        result = make_test_tool(tree, TestSettings(IMAGE, scripts, exec_scratch=enabled))()
    assert result['status'] == 'completed'
    args = json.loads((scripts / 'submitted.json').read_text())
    action = args[args.index('--') + 1:]
    assert ('--exec-scratch' in action) is enabled


@pytest.mark.parametrize("mode", [0o755, 0o600, 0o4750])
def test_staging_preserves_permissions_without_special_bits(submission, tmp_path, mode):
    from tl3_workers.pb_tests import _stage

    source = submission[0] / "script"
    source.write_text("#!/bin/sh\nexit 0\n")
    source.chmod(mode)
    staged = tmp_path / "staged"
    staged.mkdir()
    with Worktree(submission[0]) as tree:
        _stage(tree, staged)
    target = staged / "checkout/script"
    assert stat.S_IMODE(target.stat().st_mode) == mode & 0o777
    assert os.access(target, os.X_OK) == bool(mode & 0o111)
    if mode & 0o100:
        assert subprocess.run([str(target)], check=False).returncode == 0
    else:
        with pytest.raises(PermissionError):
            subprocess.run([str(target)], check=False)
    indexed = subprocess.check_output(["git", "ls-files", "--stage", "checkout/script"], cwd=staged)
    assert indexed.startswith(b"100755" if mode & 0o100 else b"100644")


def test_base_run_returns_base_outcomes_and_keeps_maker_tree(submission):
    root, scripts = submission

    def git(*args):
        return subprocess.check_output([
            "git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false",
            "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", *args,
        ], cwd=root, text=True).strip()

    git("init", "--quiet", "--template=")
    (root / "example_module.py").write_text("value = 'base'\n")
    (root / "test_code.py").write_text("from example_module import value\ndef test_value(): assert value == 'head'\n")
    git("add", ".")
    git("commit", "--quiet", "-m", "Check base")
    base = git("rev-parse", "HEAD")
    (root / "example_module.py").write_text("value = 'earlier maker commit'\n")
    git("add", ".")
    git("commit", "--quiet", "-m", "Earlier maker round")
    head = git("rev-parse", "HEAD")
    (root / "example_module.py").write_text("value = 'head'\n")
    before = git("status", "--porcelain")
    script = scripts / "pbrun.py"
    script.write_text(script.read_text().replace(
        'assert (root / "checkout/uncommitted.py").read_bytes() == b"current dirty bytes\\x00\\xff"\n', "",
    ).replace(
        'print(json.dumps({"schema":',
        '''import subprocess
sys.path.insert(0, str(root / "trusted"))
from tl3_harness.test_tool import Summary
selected = args[args.index("--") + 6]
run = subprocess.run([sys.executable, str(root / "trusted/tl3_harness/runner.py"),
                      selected, str(root / "checkout"), "--summary"], capture_output=True, text=True)
assert run.returncode in (0, 1), run.stdout + run.stderr
summary = Summary()
for line in run.stdout.splitlines():
    record = json.loads(line)
    if record["event"] == "result":
        summary.add(record["test"], record["outcome"], record["message"])
(here / "response.json").write_text(json.dumps(summary.result))
print(json.dumps({"schema":''',
    ))
    with Worktree(root) as tree:
        tree.test_checkout = root
        tree.check_base_sha = base
        tool = make_test_tool(tree, TestSettings(IMAGE, scripts))
        current = tool(["test_code.py::test_value"])
        baseline = tool(["test_code.py::test_value"], base=True)
    assert current["totals"]["passed"] == 1, current
    assert baseline["totals"]["failed"] == 1, baseline
    assert baseline["tests"][0]["test"] == "test_code.py::test_value"
    assert baseline["action_key"] == KEY
    assert baseline["base_sha"] == base
    assert git("rev-parse", "HEAD") == head
    assert git("status", "--porcelain") == before
    assert (root / "example_module.py").read_text() == "value = 'head'\n"
