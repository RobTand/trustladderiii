import hashlib
import json
import os
from pathlib import Path
import re
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
import subprocess
(here / "tree.txt").write_text(subprocess.run(["git", "write-tree", "--prefix=checkout/"], cwd=root,
                                              capture_output=True, text=True, check=True).stdout.strip())
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
        assert set(tool.args) == {"tests", "base", "repeat", "parallel", "priority"}
        return tool.invoke({} if tests is None else {"tests": tests})


@pytest.mark.parametrize("outcome", ["passed", "failed", "error", "skipped"])
def test_fake_submission_returns_outcomes_and_uses_only_test_action(submission, outcome):
    summary = Summary()
    summary.add("tests/test_code.py::test_value", outcome, "fixture failure" if outcome == "failed" else "")
    result = call(submission, summary.result, ["tests/test_code.py::test_value"])
    log = (submission[1] / "action.log").read_bytes()
    expected = {**summary.result, "action_key": KEY, "receipts": [{
        "action_key": KEY, "status": "executed", "exit_code": 0, "receipt_sha256": "b" * 64,
        "log_sha256": hashlib.sha256(log).hexdigest()}], "tree": (submission[1] / "tree.txt").read_text()}
    if outcome in ("failed", "error"):
        summary.result["tests"][0].update(classification="unverified", base_outcome="unverified")
        expected["base"] = {key: value for key, value in infrastructure(
            "The role did not supply a check base and checkout.").items() if key != "tests"}
    assert result == expected
    args = json.loads((submission[1] / "submitted.json").read_text())
    assert "--detach" in args and "--anywhere" in args
    environment = [args[index + 1] for index, arg in enumerate(args) if arg == "--env"]
    assert environment == ["OMP_NUM_THREADS=1", "MKL_NUM_THREADS=1", "OPENBLAS_NUM_THREADS=1"]
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
            operation = next(i for i, arg in enumerate(command) if arg in ("init", "fetch", "add", "commit", "update-ref", "write-tree"))
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


@pytest.mark.parametrize("mode", [0o755, 0o664, 0o600, 0o700, 0o777, 0o750, 0o4750, 0o610])
def test_staging_modes_match_commit_export(submission, tmp_path, mode):
    from tl3_harness.trees import export
    from tl3_workers.pb_tests import _stage

    _, _, _, git = prepare_base_submission(submission)
    source = submission[0] / "script"
    source.write_text("#!/bin/sh\nexit 0\n")
    source.chmod(mode)
    git("add", "--force", "--all")
    git("commit", "--quiet", "-m", "Same content as maker staging")
    exported = tmp_path / "exported"
    export(submission[0], git("rev-parse", "HEAD"), exported)
    staged = tmp_path / "staged"
    staged.mkdir()
    with Worktree(submission[0]) as tree:
        _stage(tree, staged)
    for path in exported.rglob("*"):
        if path.is_file():
            target = staged / "checkout" / path.relative_to(exported)
            assert target.read_bytes() == path.read_bytes()
            assert stat.S_IMODE(target.stat().st_mode) == stat.S_IMODE(path.stat().st_mode)
    target = staged / "checkout/script"
    assert stat.S_IMODE(target.stat().st_mode) == (0o755 if mode & 0o100 else 0o644)
    assert os.access(target, os.X_OK) == bool(mode & 0o100)
    if mode & 0o100:
        assert subprocess.run([str(target)], check=False).returncode == 0
    else:
        with pytest.raises(PermissionError):
            subprocess.run([str(target)], check=False)
    indexed = subprocess.check_output(["git", "ls-files", "--stage", "checkout/script"], cwd=staged)
    assert indexed.startswith(b"100755" if mode & 0o100 else b"100644")


def prepare_base_submission(submission, base_value="base", head_value="head"):
    root, scripts = submission

    def git(*args):
        return subprocess.check_output([
            "git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false",
            "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", *args,
        ], cwd=root, text=True).strip()

    git("init", "--quiet", "--template=")
    (root / "example_module.py").write_text(f"value = {base_value!r}\n")
    test_source = ("from example_module import value\ndef test_value():\n"
                   "    if value == 'error': raise RuntimeError('fixture error')\n"
                   "    assert value == 'head'\n"
                   "def test_other(): assert value == 'head'\ndef test_pass(): pass\n")
    if base_value != "missing":
        (root / "test_code.py").write_text(test_source)
    git("add", ".")
    git("commit", "--quiet", "-m", "Check base")
    base = git("rev-parse", "HEAD")
    (root / "test_code.py").write_text(test_source)
    (root / "example_module.py").write_text("value = 'earlier maker commit'\n")
    git("add", ".")
    git("commit", "--quiet", "-m", "Earlier maker round")
    head = git("rev-parse", "HEAD")
    (root / "example_module.py").write_text(f"value = {head_value!r}\n")
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
assert run.returncode in (0, 1, 4, 5), run.stdout + run.stderr
summary = Summary()
for line in run.stdout.splitlines():
    record = json.loads(line)
    if record["event"] == "result":
        summary.add(record["test"], record["outcome"], record["message"])
(here / "response.json").write_text(json.dumps(summary.result))
record = here / "submissions.json"
submissions = json.loads(record.read_text()) if record.exists() else []
submissions.append(json.loads(selected))
record.write_text(json.dumps(submissions))
print(json.dumps({"schema":''',
    ).replace('"action_key": "a" * 64', '"action_key": ("a" if len(submissions) == 1 else "c") * 64'))
    waiter = scripts / "pbwait.py"
    waiter.write_text(waiter.read_text().replace('"action_key": "a" * 64', '"action_key": sys.argv[-1]'))
    return base, head, before, git


def test_base_run_returns_base_outcomes_and_keeps_maker_tree(submission):
    root, scripts = submission
    base, head, before, git = prepare_base_submission(submission)
    with Worktree(root) as tree:
        tree.test_checkout = root
        tree.check_base_sha = base
        tool = make_test_tool(tree, TestSettings(IMAGE, scripts))
        current = tool(["test_code.py::test_value"])
        baseline = tool(["test_code.py::test_value"], base=True)
    assert current["totals"]["passed"] == 1, current
    assert baseline["totals"]["failed"] == 1, baseline
    assert baseline["tests"][0]["test"] == "test_code.py::test_value"
    assert baseline["action_key"] == "c" * 64
    assert baseline["base_sha"] == base
    assert git("rev-parse", "HEAD") == head
    assert git("status", "--porcelain") == before
    assert (root / "example_module.py").read_text() == "value = 'head'\n"


@pytest.mark.parametrize("base_value,head_value,classification,actions", [
    ("base", "broken", "pre-existing", 2),
    ("head", "broken", "new", 2),
    ("error", "error", "pre-existing", 2),
    ("head", "error", "new", 2),
    ("missing", "broken", "new", 2),
    ("base", "head", None, 1),
])
def test_automatic_base_comparison(submission, base_value, head_value, classification, actions):
    root, scripts = submission
    base, head, before, git = prepare_base_submission(submission, base_value, head_value)
    tests = ["test_code.py::test_value", "test_code.py::test_other", "test_code.py::test_pass"]
    with Worktree(root) as tree:
        tree.test_checkout = root
        tree.check_base_sha = base
        tree.test_checks = [{"kind": "regression", "tests": tests[:2]}]
        result = make_test_tool(tree, TestSettings(IMAGE, scripts))(tests)
    submitted = json.loads((scripts / "submissions.json").read_text())
    assert len(submitted) == actions, result
    assert result["action_key"] == KEY
    if classification:
        assert submitted[1] == tests[:2], "batch only failing tests into one base action"
        failed = [test for test in result["tests"] if test["test"] in tests[:2]]
        assert all(test["classification"] == classification for test in failed)
        assert result["base"]["action_key"] == "c" * 64
        assert result["base"]["base_sha"] == base
    else:
        assert "base" not in result
    assert git("rev-parse", "HEAD") == head
    assert git("status", "--porcelain") == before


@pytest.mark.parametrize("kind", ["regression", "demonstrating", "reproducing", "behavior-preserving", None])
@pytest.mark.parametrize("base_value", ["base", "error"])
def test_base_failure_classification_respects_check_kind(submission, kind, base_value):
    root, scripts = submission
    base, _, _, _ = prepare_base_submission(submission, base_value, "broken")
    test = "test_code.py::test_value"
    with Worktree(root) as tree:
        tree.test_checkout = root
        tree.check_base_sha = base
        tree.test_checks = [{"kind": kind, "tests": [test]}] if kind else []
        result = make_test_tool(tree, TestSettings(IMAGE, scripts))([test])
    assert result["tests"][0]["base_outcome"] == ("error" if base_value == "error" else "failed")
    assert result["tests"][0]["classification"] == (
        "pre-existing" if kind == "regression" else "fails-on-base"
    )
    assert len(json.loads((scripts / "submissions.json").read_text())) == 2


@pytest.mark.parametrize("selector", ["test_code.py::test_value", "test_code.py::test_value[0]"])
def test_regression_classification_matches_parametrized_selector(submission, selector):
    root, scripts = submission
    _, _, _, git = prepare_base_submission(submission)
    (root / "test_code.py").write_text(
        "import pytest\n@pytest.mark.parametrize('case', [0, 1])\n"
        "def test_value(case): assert False\n"
        "def test_value_extra(): assert False\n"
    )
    git("add", "--all")
    git("commit", "--quiet", "-m", "Parametrized check base")
    with Worktree(root) as tree:
        tree.test_checkout = root
        tree.check_base_sha = git("rev-parse", "HEAD")
        tree.test_checks = [{"kind": "regression", "tests": [selector]}]
        result = make_test_tool(tree, TestSettings(IMAGE, scripts))(["test_code.py"])
    assert {item["test"]: item["classification"] for item in result["tests"]} == {
        "test_code.py::test_value[0]": "pre-existing",
        "test_code.py::test_value[1]": "pre-existing" if selector.endswith("test_value") else "fails-on-base",
        "test_code.py::test_value_extra": "fails-on-base",
    }


def test_comparison_truncation_keeps_failures_before_passes(submission):
    summary = Summary()
    for i in range(60):
        summary.add(f"test_code.py::test_pass_{i}", "passed", "p" * 90)
    for i in range(30):
        summary.add(f"test_code.py::test_fail_{i}", "failed", "f" * 90)
    failures = [item["test"] for item in summary.result["tests"] if item["outcome"] == "failed"]
    assert failures and not summary.result["note"], "start with a complete head summary"
    result = call(submission, summary.result)
    assert len(json.dumps(result).encode()) <= SUMMARY_BYTES
    assert "Truncated" in result["note"]
    assert [item["test"] for item in result["tests"] if item["outcome"] == "failed"] == failures
    assert sum(item["outcome"] == "passed" for item in result["tests"]) < 60
    assert result["totals"] == summary.result["totals"]


def invoke(submission, response, arguments):
    root, scripts = submission
    (scripts / "response.json").write_text(json.dumps(response))
    with Worktree(root) as tree:
        tree.test_settings = TestSettings(IMAGE, scripts, cpus=1, mem_gb=2)
        tool = build_tools(tree, {"run_tests": "Run worktree tests through PrismaBuild."})[0]
        return tool.invoke(arguments), tree


def submitted(submission):
    return json.loads((submission[1] / "submitted.json").read_text())


def test_default_submission_keeps_its_action_identity(submission):
    summary = Summary()
    summary.add("tests/test_code.py::test_value", "passed")
    invoke(submission, summary.result, {"tests": ["tests/test_code.py::test_value"]})
    args = submitted(submission)
    assert "--priority" not in args
    action = args[args.index("--") + 1:]
    assert "--repeat" not in action and "--parallel" not in action and "--fresh" not in action


def test_repeat_parallel_and_priority_reach_the_action_and_reservation(submission):
    # #408: five fresh runs, one xdist run, at the request's priority.
    summary = Summary()
    summary.add("tests/test_code.py::test_value", "passed")
    invoke(submission, summary.result, {"tests": ["tests/test_code.py::test_value"],
                                        "repeat": 5, "parallel": 2, "priority": -10})
    args = submitted(submission)
    assert args[args.index("--priority") + 1] == "-10"
    assert args[args.index("--cpus") + 1] == "2", "each xdist worker reserves a core"
    assert args[args.index("--demand") + 1] == "mem_gb=4"
    action = args[args.index("--") + 1:]
    assert action[action.index("--repeat") + 1] == "5"
    assert action[action.index("--parallel") + 1] == "2"
    assert "--timeout-s" not in args, "repeat stays inside PrismaBuild's progress contract"


@pytest.mark.parametrize("arguments", [
    {"repeat": 0}, {"repeat": 11}, {"repeat": True}, {"parallel": 1}, {"parallel": 5},
    {"priority": 1}, {"priority": -11},
])
def test_options_are_bounded(submission, arguments):
    with Worktree(submission[0]) as tree:
        tool = make_test_tool(tree, TestSettings(IMAGE, submission[1]))
        with pytest.raises(ValueError, match="repeat|parallel|priority"):
            tool(**arguments)
    assert not (submission[1] / "submitted.json").exists()


def test_results_carry_receipts(submission):
    summary = Summary()
    summary.add("tests/test_code.py::test_value", "passed")
    result, tree = invoke(submission, summary.result, {"tests": ["tests/test_code.py::test_value"],
                                                      "repeat": 2})
    log = (submission[1] / "action.log").read_bytes()
    assert result["receipts"] == [{
        "action_key": KEY, "status": "executed", "exit_code": 0,
        "receipt_sha256": "b" * 64,
        "log_sha256": hashlib.sha256(log).hexdigest(),
    }]
    # The maker cites them and the reviewer sees them, bound to the tree PrismaBuild ran (#432).
    evidence = tree.evidence.test_evidence()
    staged = (submission[1] / "tree.txt").read_text()
    assert re.fullmatch("[0-9a-f]{40}|[0-9a-f]{64}", staged)
    assert evidence == [{"tests": ["tests/test_code.py::test_value"], "base": False,
                         "repeat": 2, "parallel": 0, "status": "completed", "tree": staged, "fresh": True,
                         "totals": summary.result["totals"], "receipts": result["receipts"]}]
    restored = type(tree.evidence)()
    restored.restore(tree.evidence.state())
    assert restored.test_evidence() == evidence


def test_repeated_runs_are_never_served_from_the_cache(submission):
    # #432: each repeat>1 call is its own action, so PrismaBuild cannot replay a stored result.
    summary = Summary()
    summary.add("tests/test_code.py::test_value", "passed")
    nonces = []
    for _ in range(2):
        invoke(submission, summary.result, {"tests": ["tests/test_code.py::test_value"], "repeat": 2})
        action = submitted(submission)[submitted(submission).index("--") + 1:]
        nonces.append(action[action.index("--fresh") + 1])
    assert nonces[0] != nonces[1] and all(re.fullmatch("[0-9a-f]{32}", nonce) for nonce in nonces)


def test_a_cache_hit_is_marked_as_not_fresh(submission):
    summary = Summary()
    summary.add("tests/test_code.py::test_value", "passed")
    (submission[1] / "override.json").write_text(json.dumps({"status": "cache_hit"}))
    result, tree = invoke(submission, summary.result, {"tests": ["tests/test_code.py::test_value"]})
    assert result["receipts"][0]["status"] == "cache_hit"
    [entry] = tree.evidence.test_evidence()
    assert entry["fresh"] is False


@pytest.fixture
def generator_repo(tmp_path):
    """A served repo with a git-index generator, like request 132's Bazel workflow."""
    root = tmp_path / "generated-repo"
    root.mkdir()
    script = '''import ast
from pathlib import Path
import subprocess
import sys
files = subprocess.check_output(["git", "ls-files", "-z"]).decode().split("\\0")
lines = []
for name in sorted(name for name in files if name.endswith(".py") and name != "generate.py"):
    imports = [node.module for node in ast.walk(ast.parse(Path(name).read_text()))
               if isinstance(node, ast.ImportFrom)]
    lines.append(name + ":" + ",".join(imports))
output = "# Generated by generate.py\\n" + "\\n".join(lines) + "\\n"
if "--check" in sys.argv:
    raise SystemExit(Path("tests/BUILD.bazel").read_text() != output)
Path("tests/BUILD.bazel").write_text(output)
'''
    (root / "generate.py").write_text(script)
    (root / "tests").mkdir()
    (root / "tests/test_wait.py").write_text("from socket import socket\n")
    (root / "tests/BUILD.bazel").write_text("# Generated by generate.py\ntests/test_wait.py:socket\n")
    for args in (("init", "--quiet", "--template="), ("add", "."),
                 ("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                  "commit", "--quiet", "-m", "Base")):
        subprocess.run(["git", *args], cwd=root, check=True, capture_output=True)
    (root / "tests/socket_support.py").write_text("def short_socket(): return 'short'\n")
    (root / "tests/test_short_socket.py").write_text("from tests.socket_support import short_socket\ndef test_short(): assert short_socket() == 'short'\n")
    (root / "tests/test_wait.py").write_text("from tests.socket_support import short_socket\n")
    return root


@pytest.fixture
def generator_backend(tmp_path, monkeypatch):
    # Exercise real git-index generation and artifact validation. Only the fleet
    # transport and Docker launch are replaced; these tests themselves run remotely.
    from tl3_harness import generator
    from tl3_workers import pb_generators
    monkeypatch.setattr(generator, "require_admission", lambda: None)
    monkeypatch.setattr(generator, "container_command", lambda tree, container, argv, cidfile: argv)
    calls = []
    artifacts = {}

    def command(args):
        if args[1].endswith("pbrun.py"):
            root = Path(args[args.index("--cwd") + 1])
            contract = json.loads((root / "generator.json").read_text())
            result = generator.run(root / "checkout", generator.Container(IMAGE), **contract)
            raw = (generator.PREFIX + json.dumps(result) + "\n").encode()
            log = tmp_path / f"generator-{len(calls)}.log"
            log.write_bytes(raw)
            artifacts[KEY] = {"action_key": KEY, "status": "executed", "exit_code": 0,
                              "succeeded": True, "result": {"verified": True,
                              "receipt_sha256": "b" * 64, "stdout": {
                                  "path": str(log), "bytes": len(raw),
                                  "sha256": hashlib.sha256(raw).hexdigest()}}}
            calls.append(args)
            return 0, {"schema": "prismabuild.submission.v1", "action_key": KEY}
        return 0, {"schema": "prismabuild.endings.v1", "complete": True,
                   "endings": [artifacts[KEY]]}

    monkeypatch.setattr(pb_generators, "_json_command", command)
    return calls, artifacts


def configured_generator(root, tmp_path, *, protected=(), files=None):
    from tl3_workers.pb_generators import make_generator_tool
    tree = Worktree(root)
    tree.test_checkout = root
    tree.test_settings = TestSettings(IMAGE, tmp_path / "tools")
    tree.generated_files = ["**/BUILD.bazel"]
    tree.generator = "python3 generate.py"
    tree.protected_paths = protected
    tree.generator_files = files or ["tests/BUILD.bazel", "tests/socket_support.py",
                                    "tests/test_short_socket.py", "tests/test_wait.py"]
    tree.generator_state = tmp_path / "generator-state.json"
    return tree, make_generator_tool(tree)


def test_request_132_git_index_generation_lands_before_independent_check(generator_repo, generator_backend, tmp_path):
    tree, generate = configured_generator(generator_repo, tmp_path)
    with tree:
        # The new files do not occur in the base Git index. The action must add them.
        result = generate()
        assert result["status"] == "completed", result
        assert result["input_tree"] != result["output_tree"]
        assert result["action_key"] == KEY
        assert result["artifact_sha256"]
        assert result["receipt_sha256"] == "b" * 64
        assert result["argv"] == ["python3", "generate.py"]
        output = (generator_repo / "tests/BUILD.bazel").read_text()
        assert "tests/socket_support.py:" in output
        assert "tests/test_short_socket.py:tests.socket_support" in output
        assert "tests/test_wait.py:tests.socket_support" in output
        assert len(json.dumps(result)) < 16_384
        subprocess.run(["git", "add", "."], cwd=generator_repo, check=True)
        subprocess.run(["python3", "generate.py", "--check"], cwd=generator_repo, check=True)
        assert generate() == result
        assert len(generator_backend[0]) == 1
        args = generator_backend[0][0]
        assert args[args.index("--timeout-s") + 1] == "180"
        assert args[args.index("--container-image") + 1] == IMAGE
        assert "--anywhere" in args and "--detach" in args


@pytest.mark.parametrize("mutation,fragment", [
    ("Path('tests/test_wait.py').write_text('escaped')", "undeclared"),
    ("Path('tests/BUILD.bazel').unlink()", "deleted"),
    ("Path('tests/BUILD.bazel').write_bytes(b'\\x00')", "text"),
    ("Path('tests/BUILD.bazel').write_bytes(b'\\xff')", "UTF-8"),
    ("Path('tests/BUILD.bazel').write_text('x' * (16 * 1048576 + 1))", "limit"),
    ("raise SystemExit(7)", "7"),
    ("Path('tests/BUILD.bazel').unlink(); Path('tests/BUILD.bazel').symlink_to('/etc/passwd')", "regular"),
    ("Path('tests/BUILD.bazel').chmod(0o755)", "mode"),
])
def test_generator_failure_and_out_of_scope_output_preserve_source(
        generator_repo, generator_backend, tmp_path, mutation, fragment):
    script = generator_repo / "generate.py"
    script.write_text(script.read_text() + "\n" + mutation + "\n")
    before = {str(path.relative_to(generator_repo)): path.read_bytes()
              for path in generator_repo.rglob('*') if path.is_file() and '.git' not in path.parts}
    tree, generate = configured_generator(generator_repo, tmp_path)
    with tree:
        result = generate()
    assert result["status"] != "completed", result
    assert fragment in result["message"], result
    assert {name: (generator_repo / name).read_bytes() for name in before} == before


@pytest.mark.parametrize("protected,files,fragment", [
    (["**/BUILD.bazel"], None, "protected"),
    ([], ["tests/socket_support.py"], "footprint"),
])
def test_generator_cannot_override_protection_or_footprint(
        generator_repo, generator_backend, tmp_path, protected, files, fragment):
    tree, generate = configured_generator(generator_repo, tmp_path, protected=protected, files=files)
    with tree:
        result = generate()
    assert result["status"] != "completed"
    assert fragment in result["message"], result


def test_generator_receipt_is_invalidated_by_source_edit(generator_repo, generator_backend, tmp_path):
    tree, generate = configured_generator(generator_repo, tmp_path)
    with tree:
        assert generate()["status"] == "completed"
        (generator_repo / "tests/socket_support.py").write_text("from pathlib import Path\n")
        assert generate()["status"] == "completed"
    assert len(generator_backend[0]) == 2
    assert "tests/socket_support.py:pathlib" in (generator_repo / "tests/BUILD.bazel").read_text()


def test_generated_outputs_cannot_be_hand_edited(generator_repo, tmp_path):
    tree, _ = configured_generator(generator_repo, tmp_path)
    with tree:
        for method, args in ((tree.write, ("tests/BUILD.bazel", "hand edit")),
                             (tree.edit, ("tests/BUILD.bazel", "socket", "hand edit"))):
            with pytest.raises(ValueError, match="generated"):
                method(*args)


def test_generator_deadline_is_a_failure_not_a_hang(generator_repo, generator_backend, tmp_path):
    from tl3_harness import generator
    import shutil
    staged = tmp_path / "staged"
    shutil.copytree(generator_repo, staged, ignore=shutil.ignore_patterns(".git"))
    result = generator.run(staged, generator.Container(IMAGE),
                           "python3 -c 'import time; time.sleep(60)'", ["**/BUILD.bazel"], [],
                           ["tests/BUILD.bazel"], timeout=0.05)
    assert result["status"] == "failed" and result["exit_code"] == 124
    assert "deadline" in result["message"]


@pytest.mark.parametrize("corruption", ["log", "output", "argv", "incomplete", "stale"])
def test_generator_refuses_unverified_or_stale_artifacts(
        generator_repo, generator_backend, tmp_path, monkeypatch, corruption):
    from tl3_workers import pb_generators
    original = pb_generators._json_command

    def command(args):
        code, value = original(args)
        if args[1].endswith("pbwait.py"):
            ending = value["endings"][0]
            log = ending["result"]["stdout"]
            path = Path(log["path"])
            if corruption == "log":
                path.write_bytes(path.read_bytes() + b"corrupt")
            elif corruption in ("output", "argv"):
                from tl3_harness.generator import PREFIX
                artifact = json.loads(path.read_text()[len(PREFIX):])
                if corruption == "output":
                    artifact["outputs"][0]["entry"]["sha256"] = "0" * 64
                else:
                    artifact["argv"] = ["/bin/sh", "-c", "exit 0"]
                raw = (PREFIX + json.dumps(artifact) + "\n").encode()
                path.write_bytes(raw)
                log.update(bytes=len(raw), sha256=hashlib.sha256(raw).hexdigest())
            elif corruption == "incomplete":
                ending.update(status="failed", succeeded=False, exit_code=137)
            else:
                (generator_repo / "tests/socket_support.py").write_text("concurrent source edit\n")
        return code, value

    monkeypatch.setattr(pb_generators, "_json_command", command)
    before = (generator_repo / "tests/BUILD.bazel").read_bytes()
    tree, generate = configured_generator(generator_repo, tmp_path)
    with tree:
        result = generate()
    assert result["status"] != "completed", result
    assert (generator_repo / "tests/BUILD.bazel").read_bytes() == before


def test_generator_reattaches_after_interrupted_import(generator_repo, generator_backend, tmp_path, monkeypatch):
    from tl3_workers import pb_generators
    original = pb_generators._import

    def interrupted(tree, outputs):
        original(tree, outputs)
        raise RuntimeError("fixture crash after atomic replacement")

    tree, generate = configured_generator(generator_repo, tmp_path)
    with tree:
        monkeypatch.setattr(pb_generators, "_import", interrupted)
        assert generate()["status"] == "failed"
        monkeypatch.setattr(pb_generators, "_import", original)
        assert generate()["status"] == "completed"
    assert len(generator_backend[0]) == 1


@pytest.mark.parametrize("overhead", [30000, 100000])
def test_request_132_maker_returns_only_the_generated_final_head(
        generator_repo, generator_backend, tmp_path, monkeypatch, overhead):
    from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
    from langchain_core.messages import AIMessage
    from tl3_workers import runtime
    from tl3_workers.contracts import validate
    for name in ("socket_support.py", "test_short_socket.py"):
        (generator_repo / "tests" / name).unlink()
    (generator_repo / "tests/test_wait.py").write_text("from socket import socket\n")
    edits = [
        ("tests/socket_support.py", "def short_socket(): return 'short'\n"),
        ("tests/test_short_socket.py", "from tests.socket_support import short_socket\ndef test_short(): assert short_socket() == 'short'\n"),
        ("tests/test_wait.py", "from tests.socket_support import short_socket\n"),
    ]
    turns = [AIMessage(content="", tool_calls=[{"name": "write", "id": str(i),
              "args": {"path": path, "content": content}}]) for i, (path, content) in enumerate(edits)]
    turns.append(AIMessage(content=json.dumps({"kind": "done", "self_check": [
        {"goal_id": "socket", "status": "met", "evidence": "Imports use shared support."}]})))
    monkeypatch.setattr(runtime, "create_model", lambda request: FakeMessagesListChatModel(responses=turns))
    request = {"protocol": 1, "role": "maker", "worktree": str(generator_repo),
               "operation": {"node": 1, "generation": 0, "kind": "make"},
               "inputs": {"brief": "Request 132: share short Unix socket support and migrate test imports.",
                          "goals": [{"id": "socket", "description": "Use shared test support."}],
                          "plan": {"files": [path for path, _ in edits] + ["tests/BUILD.bazel"],
                                   "checks": [{"name": "support", "kind": "demonstrating",
                                               "tests": ["tests/test_short_socket.py::test_short"],
                                               "goal_ids": ["socket"]}]},
                          "forecast": {"fit_tokens": 100000, "context_tokens": 150000,
                                       "overhead_tokens": {"work": overhead},
                                       "generated_files": ["**/BUILD.bazel"],
                                       "generator": "python3 generate.py", "generator_available": True},
                          "attempt": 1, "round": 0, "context_size": 150000,
                          "tests": {"image": IMAGE, "tools_path": str(tmp_path / "tools")}}}
    validate("maker-input", request["inputs"])
    events = []
    runtime.run(request, events.append)
    result = events[-1]["reply"]
    if overhead == 100000:
        assert result["kind"] == "cant_meet" and "final fit" in result["report"], result
        assert "head" not in result
        return
    assert result["kind"] == "done", result
    validate("maker-reply", result)
    tree = subprocess.check_output(["git", "rev-parse", result["head"] + "^{tree}"], cwd=generator_repo).decode().strip()
    assert result["generation_evidence"]["output_tree"] == tree
    assert result["generation_evidence"]["action_key"] == KEY
    subprocess.run(["python3", "generate.py", "--check"], cwd=generator_repo, check=True)
    assert "tests/socket_support.py:" in subprocess.check_output(
        ["git", "show", result["head"] + ":tests/BUILD.bazel"], cwd=generator_repo).decode()
    assert events[-1]["event"] == "reply"


def test_large_generated_output_uses_verified_artifact_without_entering_model_context(
        generator_repo, generator_backend, tmp_path):
    script = generator_repo / "generate.py"
    script.write_text(script.read_text() + "\nPath('tests/BUILD.bazel').write_text('# Generated by fixture\\n' + 'x' * 6_900_000)\n")
    tree, generate = configured_generator(generator_repo, tmp_path)
    with tree:
        result = generate()
        assert result["status"] == "completed", result
        assert len(json.dumps(result).encode()) < 16_384
        assert len(tree.read("tests/BUILD.bazel").encode()) < 3072
    assert (generator_repo / "tests/BUILD.bazel").stat().st_size > 6_900_000


def test_generator_tool_has_no_model_selected_command_or_arguments(generator_repo, tmp_path):
    tree, _ = configured_generator(generator_repo, tmp_path)
    with tree:
        tool = build_tools(tree, {"run_generator": "Run the configured admitted generator."})[0]
        assert tool.args == {}


def test_generator_image_command_cannot_be_shadowed_by_a_repository_file(tmp_path):
    from tl3_harness import generator
    (tmp_path / "python3").write_text("#!/usr/bin/env bash\nexit 7\n")
    (tmp_path / "python3").chmod(0o755)
    command = generator.container_command(tmp_path, generator.Container(IMAGE),
                                          ["python3", "generate.py"], tmp_path / "cid")
    assert "--entrypoint=python3" in command
    assert command[-2:] == [IMAGE, "generate.py"]


@pytest.mark.parametrize("before_mode,after_mode", [(0o644, 0o755), (0o755, 0o644), (None, 0o755)])
def test_generator_rejects_mode_changes_in_action_and_artifact_validation(before_mode, after_mode):
    import base64
    from tl3_harness import generator
    name, raw = "tests/BUILD.bazel", b"generated\n"
    entry = {"sha256": hashlib.sha256(raw).hexdigest(), "bytes": len(raw), "mode": after_mode}
    before = {name: {**entry, "mode": before_mode}} if before_mode is not None else {}
    after = {name: entry}
    contract = {"command": "python3 generate.py", "generated_files": ["**/BUILD.bazel"],
                "protected_paths": [], "files": [name]}
    artifact = {"status": "completed", "exit_code": 0, "command": contract["command"],
                "argv": ["python3", "generate.py"], "input_manifest": generator.digest(before),
                "output_manifest": generator.digest(after),
                "outputs": [{"path": name, "entry": entry, "data": base64.b64encode(raw).decode()}]}
    with pytest.raises(ValueError, match="mode"):
        generator.changed(before, after, **{key: contract[key] for key in
                                           ("generated_files", "protected_paths", "files")})
    with pytest.raises(ValueError, match="mode"):
        generator.validate_artifact(artifact, before, contract)


@pytest.mark.parametrize("entrypoint", ["./generate.py", "tools/generate.py", "/checkout/generate.py",
                                       "/proc/self/cwd/generate.py"])
def test_generator_never_resolves_a_maker_written_shebang(tmp_path, entrypoint):
    from tl3_harness import generator
    script = tmp_path / ("generate.py" if entrypoint.startswith("/") else entrypoint.removeprefix("./"))
    script.parent.mkdir(parents=True, exist_ok=True)
    script.write_text("#!/bin/sh -c 'printf changed > tests/BUILD.bazel'\n")
    script.chmod(0o755)
    with pytest.raises(ValueError, match="configured.*interpreter"):
        generator.container_command(tmp_path, generator.Container(IMAGE), [entrypoint], tmp_path / "cid")
    command = generator.container_command(tmp_path, generator.Container(IMAGE),
                                          ["python3", str(script.relative_to(tmp_path))], tmp_path / "cid")
    assert "--entrypoint=python3" in command
    assert command[-2:] == [IMAGE, str(script.relative_to(tmp_path))]
