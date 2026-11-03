from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys

import pytest

from tl3_workers.forcing_rehearsal import ForcingRehearsals
from tl3_workers.roles import planner
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


@pytest.mark.parametrize("case,expected", [
    ("edited", "pass"), ("unchanged", "fail"), ("broken_head", "fail"),
    ("import_error", "fail"), ("setup_error", "fail"), ("missing_selector", "fail"),
    ("configuration", "fail"), ("protected", "fail"), ("helper_only", "fail"),
])
def test_request_158_plain_base_is_advisory_and_checking_owns_overlay(
        submission, tmp_path, monkeypatch, case, expected):
    import sys
    from tl3_harness import execution
    from tl3_harness.model import Check, Kind, Outcome
    from tl3_workers.pb_tests import _git

    root, scripts = submission
    (root / "pyproject.toml").write_text(
        '[tool.tl3.harness]\ncommand=["python3", "-m", "pytest"]\ntest-files=["test_*.py"]\n')
    base, _, _, git = prepare_base_submission(submission, base_value="head", head_value="fixed")
    test = "test_code.py::test_value"
    source = (root / "test_code.py").read_text()
    if case == "unchanged":
        (root / "example_module.py").write_text("value = 'head'\n")
    if case != "unchanged":
        source = source.replace("assert value == 'head'", "assert value in ('head', 'fixed')\n    assert value == 'fixed'", 1)
    if case == "broken_head":
        (root / "example_module.py").write_text("value = 'broken'\n")
    elif case == "import_error":
        source = "import nonexistent_request_158_module\n" + source
    elif case == "setup_error":
        source = "import pytest\n@pytest.fixture(autouse=True)\ndef setup(): assert False\n" + source
    elif case == "missing_selector":
        test = "test_code.py::test_absent"
    elif case == "configuration":
        (root / "pytest.ini").write_text("[pytest]\n")
    elif case == "helper_only":
        # Whole-file overlay carries the helper fix too. It cannot manufacture RED.
        (root / "example_module.py").write_text("value = 'head'\n")
        source = "def helper(): return 'fixed'\ndef test_value(): assert helper() == 'fixed'\n"
    (root / "test_code.py").write_text(source)
    with Worktree(root) as tree:
        tree.test_checkout, tree.check_base_sha = root, base
        tool = make_test_tool(tree, TestSettings(IMAGE, scripts))
        baseline = tool([test], base=True)
        if case == "edited":
            (root / "example_module.py").write_text("value = 'head'\n")
            red = tool([test])
            assert red["totals"]["failed"] == 1, red
            assert "assert value == 'fixed'" in red["tests"][0]["message"]
            assert red["tests"][0]["base_outcome"] == "passed"
            (root / "example_module.py").write_text("value = 'fixed'\n")
        current = tool([test])
    git("add", ".")
    git("commit", "--quiet", "-m", "Request 158 edited existing test")
    head = git("rev-parse", "HEAD")
    # Only the container transport is replaced: real staging, selection, runner,
    # report parsing and judging all execute inside the outer admitted PB job.
    runner = Path(execution.__file__).with_name("runner.py")
    monkeypatch.setattr(execution, "require_admission", lambda: None)
    monkeypatch.setenv("PRISMABUILD_CONTAINER_OWNER", str(tmp_path))
    monkeypatch.setenv("PRISMABUILD_ACTION_PROGRESS_PATH", str(tmp_path / "progress.json"))
    monkeypatch.setenv("PRISMABUILD_ACTION_PROGRESS_TOKEN", "fixture-token")
    observed = {}
    def command(staged, container, tests):
        observed[staged.name] = _git(root, "hash-object", str(staged / "test_code.py")).strip()
        return [sys.executable, "-I", "-B", str(runner), "-", str(staged)]
    monkeypatch.setattr(execution, "container_command", command)
    [result] = execution.run_checks(
        root, base, head, (Check("fallback", Kind.DEMONSTRATING, (test,)),),
        container=execution.Container(IMAGE), protected=("example_module.py",) if case == "protected" else ())
    assert result.verdict == expected, result
    if case == "edited":
        assert baseline["totals"]["passed"] == 1, baseline
        assert current["totals"]["passed"] == 1, current
        assert result.tests[0].base == Outcome.ASSERTION
        assert result.tests[0].head == Outcome.PASSED
        assert observed["demonstrating"] == observed["head"]
    assert baseline["base_semantics"] == "untouched_base"
    assert baseline["demonstrating_proof"] == "pending_checking"


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


@pytest.mark.parametrize("parallel", [0, 2])
def test_configured_demand_reaches_maker_head_and_base_actions(submission, monkeypatch, parallel):
    from tl3_workers import pb_tests
    commands = []
    original = pb_tests._json_command
    def command(args):
        if "--cwd" in args:
            commands.append(args)
        return original(args)
    monkeypatch.setattr(pb_tests, "_json_command", command)
    root, scripts = submission
    base, _, _, _ = prepare_base_submission(submission, head_value="broken")
    with Worktree(root) as tree:
        tree.test_settings = TestSettings(IMAGE, scripts, cpus=4, mem_gb=12)
        tree.check_base_sha = base
        tree.test_checkout = root
        result = make_test_tool(tree)(["test_code.py::test_value"], parallel=parallel)
    assert result["status"] == "completed"
    assert len(commands) == 2, "head failure automatically submits the base"
    for index, args in enumerate(commands):
        assert args[args.index("--cpus") + 1] == "4"
        assert args[args.index("--demand") + 1] == f"mem_gb={12 * (max(1, parallel) if index == 0 else 1)}"
        for name in ("OMP_NUM_THREADS", "MKL_NUM_THREADS", "OPENBLAS_NUM_THREADS"):
            assert f"{name}={max(1, 4 // parallel) if parallel and index == 0 else 4}" in args


def test_oom_retains_allowlisted_failure_receipt(submission):
    result = call(submission, Summary().result, override={
        "status": "failed", "succeeded": False, "exit_code": 137,
        "reason": "memory_limit_oom",
        "diagnostics": {"termination_reason": "memory_limit_oom",
                        "resource_profile": {"scope": {"memory_peak_bytes": 2147483648}},
                        "admission_private": "never retain"},
    })
    assert result["status"] == "infrastructure_error"
    receipt = result["receipts"][0]
    assert receipt == {"action_key": KEY, "status": "failed", "exit_code": 137,
                       "termination_reason": "memory_limit_oom", "memory_peak_bytes": 2147483648}
    assert "memory_limit_oom" in result["message"]


@pytest.mark.parametrize("publication, recorded_key, expected", [
    (123, KEY, True), (122, KEY, False), (123, "c" * 64, False),
])
def test_legacy_ending_retains_only_matching_publication_oom(submission, publication, recorded_key, expected):
    # pbwait currently returns RC/peak but omits the exact termination reason.
    root, scripts = submission
    installed_tools = root.parent / "fleet/repo/tools"
    installed_tools.mkdir(parents=True)
    for name in ("pbrun.py", "pbwait.py"):
        (installed_tools / name).write_text((scripts / name).read_text())
    (installed_tools / "response.json").write_text(json.dumps(Summary().result))
    (installed_tools / "override.json").write_text(json.dumps({
        "status": "failed", "succeeded": False, "exit_code": None,
        "reason": "action ended failed", "publication": {"published_unix": 123},
        "diagnostics": {"returncode": 137,
                        "resource_profile": {"scope": {"memory_peak_bytes": 2147483648}}},
    }))
    failed = root.parent / "fleet/pb-queue/failed"
    failed.mkdir(parents=True)
    (failed / f"{KEY}.json").write_text(json.dumps({
        "action_key": recorded_key, "published_unix": publication,
        "detail": {"termination_reason": "memory_limit_oom", "private": "not copied"},
    }))
    with Worktree(root) as tree:
        result = make_test_tool(tree, TestSettings(IMAGE, installed_tools))()
    assert result["status"] == "infrastructure_error"
    receipt = result["receipts"][0]
    assert receipt["exit_code"] == 137
    assert receipt["memory_peak_bytes"] == 2147483648
    assert (receipt.get("termination_reason") == "memory_limit_oom") is expected
    assert "private" not in json.dumps(result)


def test_sigkill_without_oom_reason_is_not_classified_as_oom(submission):
    result = call(submission, Summary().result, override={
        "status": "failed", "succeeded": False, "exit_code": 137,
        "reason": "action ended failed",
    })
    assert result["receipts"] == [{"action_key": KEY, "status": "failed", "exit_code": 137}]
    assert "memory_limit_oom" not in result["message"]
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
                              "diagnostics": {"elapsed_s": 123.5},
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
])
def test_generator_cannot_override_protection(
        generator_repo, generator_backend, tmp_path, protected, files, fragment):
    tree, generate = configured_generator(generator_repo, tmp_path, protected=protected, files=files)
    with tree:
        result = generate()
    assert result["status"] != "completed"
    assert fragment in result["message"], result



def test_request_2709_test_only_plan_imports_configured_output(generator_repo, generator_backend, tmp_path):
    for name in ("socket_support.py", "test_short_socket.py"):
        (generator_repo / "tests" / name).unlink()
    (generator_repo / "tests/test_wait.py").write_text("from pathlib import Path\n")
    tree, generate = configured_generator(generator_repo, tmp_path, files=["tests/test_wait.py"])
    with tree:
        result = generate()
        assert result["status"] == "completed", result
        assert "tests/test_wait.py:pathlib" in (generator_repo / "tests/BUILD.bazel").read_text()
        assert generate() == result, "verified import remains idempotent"
    assert len(generator_backend[0]) == 1


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


@pytest.mark.parametrize("cpus,mem_gb", [(1, 2), (4, 12)])
def test_generator_uses_repository_demand_and_native_thread_budget(
        generator_repo, generator_backend, tmp_path, cpus, mem_gb):
    tree, generate = configured_generator(generator_repo, tmp_path)
    tree.test_settings = TestSettings(IMAGE, tmp_path / "tools", cpus=cpus, mem_gb=mem_gb)
    with tree:
        assert generate()["status"] == "completed"
    [args] = generator_backend[0]
    assert args[args.index("--cpus") + 1] == str(cpus)
    assert args[args.index("--demand") + 1] == f"mem_gb={mem_gb}"
    assert tree.generation_receipt.get("budget", {}).get("cpus") == cpus
    assert tree.generation_receipt["budget"]["mem_gb"] == mem_gb
    for name in ("OMP_NUM_THREADS", "MKL_NUM_THREADS", "OPENBLAS_NUM_THREADS"):
        assert f"{name}={cpus}" in args


def test_generator_oom_retains_failure_receipt(generator_repo, tmp_path, monkeypatch):
    from tl3_workers import pb_generators
    ending = {"action_key": KEY, "status": "failed", "exit_code": 137, "succeeded": False,
              "diagnostics": {"termination_reason": "memory_limit_oom",
                              "resource_profile": {"scope": {"memory_peak_bytes": 2147483648}},
                              "private": "not retained"}}
    def command(args):
        if args[1].endswith("pbrun.py"):
            return 0, {"schema": "prismabuild.submission.v1", "action_key": KEY}
        return 1, {"schema": "prismabuild.endings.v1", "complete": True, "endings": [ending]}
    monkeypatch.setattr(pb_generators, "_json_command", command)
    tree, generate = configured_generator(generator_repo, tmp_path)
    with tree:
        result = generate()
    assert result["status"] == "failed"
    assert result["receipts"] == [{"action_key": KEY, "status": "failed", "exit_code": 137,
                                   "termination_reason": "memory_limit_oom", "memory_peak_bytes": 2147483648}]
    assert "memory_limit_oom" in result["message"]
    assert "private" not in json.dumps(result)
    assert tree.generation_receipt == result


# Request160: successful adoption bypasses the slow decision method.
NODE = 'tests/test_adoption.py::test_adopt'
FORCING = {'symptom': 'Publishing waits for a slow decision.', 'environment': {},
           'patches': [{'target': 'publisher.Publisher.decide', 'delay_seconds': 0.001}]}
SOURCE = '''
class Publisher:
    def decide(self):
        return True

    def try_adopt(self):
        return True

    def publish(self):
        if self.try_adopt():
            return True
        return self.decide()
'''
TEST = '''
from publisher import Publisher
import time

def test_adopt():
    start = time.monotonic()
    assert Publisher().publish()
    assert time.monotonic() - start < 0.5
'''


def git(root, *args):
    return subprocess.run(['git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                           *args], cwd=root, check=True, capture_output=True, text=True).stdout.strip()


@pytest.fixture
def rehearsal(tmp_path, monkeypatch):
    root = tmp_path / 'repo'
    (root / 'tests').mkdir(parents=True)
    (root / 'publisher.py').write_text(SOURCE)
    (root / 'tests/test_adoption.py').write_text(TEST)
    (root / 'pyproject.toml').write_text('[tool.tl3.harness]\ntest-files=["tests/*.py"]\n')
    git(root, 'init', '-q')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'base')
    base = git(root, 'rev-parse', 'HEAD')
    inputs = {'base_sha': base, 'tests': {'image': IMAGE, 'tools_path': str(tmp_path)},
              'must_split': False, 'findings': [], 'protected_paths': [],
              'ingest': {'brief': 'Preserve adoption, no copying and no publication wait.',
                         'goals': [{'description': 'Make the adoption test deterministic.', 'verification': 'test'}]},
              'forecast': {'fit_tokens': 100000, 'context_tokens': 150000, 'overhead_tokens': {'tools': 1}}}
    calls = []
    from tl3_workers import pb_tests

    def submit(command):
        if 'pbrun.py' in command[1]:
            calls.append(command)
            staged = Path(command[command.index('--cwd') + 1])
            # A fake PB transport drives the real staged harness and runner on free remote compute.
            # Only the admission/container seams are replaced; the served code is never mocked.
            script = '''
import sys
from pathlib import Path
sys.path.insert(0, str(Path('trusted').resolve()))
from tl3_harness import test_tool
from tl3_harness import execution
import json
execution.require_admission = test_tool.require_admission = lambda: None
import os
os.environ['PRISMABUILD_ACTION_PROGRESS_PATH'] = str(Path('progress.json').resolve())
os.environ['PRISMABUILD_ACTION_PROGRESS_TOKEN'] = 'fixture'
test_tool.container_command = lambda tree, container, tests: [sys.executable, '-I', '-B',
    str(Path(execution.__file__).with_name('runner.py')), '-', str(tree)]
sys.argv = ['action.py', *json.loads(sys.argv[1])]
test_tool.main()
'''
            args = command[command.index('action.py') + 1:]
            child = subprocess.run([sys.executable, '-c', script, json.dumps(args)], cwd=staged,
                                   capture_output=True, text=True, check=True)
            log = tmp_path / 'action.log'
            log.write_text(child.stdout)
            return 0, {'schema': 'prismabuild.submission.v1', 'action_key': 'a' * 64}
        import hashlib
        log = tmp_path / 'action.log'
        raw = log.read_bytes()
        return 0, {'schema': 'prismabuild.endings.v1', 'complete': True, 'endings': [
            {'action_key': 'a' * 64, 'status': 'executed', 'exit_code': 0, 'succeeded': True,
             'result': {'verified': True, 'receipt_sha256': 'b' * 64,
                        'stdout': {'path': str(log), 'bytes': len(raw),
                                   'sha256': hashlib.sha256(raw).hexdigest()}}}]}
    monkeypatch.setattr(pb_tests, '_json_command', submit)
    monkeypatch.setattr(planner.Forecaster, 'measure', lambda *a: {'fits': True})
    monkeypatch.chdir(root)
    with Worktree(root) as tree:
        yield ForcingRehearsals(tree, inputs), root, inputs, calls


def plan(forcing=FORCING):
    return {'kind': 'leaf', 'files': ['tests/test_adoption.py'], 'edits_tests': [NODE],
            'checks': [{'kind': 'demonstrating', 'node_ids': [NODE], 'goal_ids': [0],
                        'forcing': deepcopy(forcing)}], 'no_design_questions': True, 'lands_on_its_own': True}


def reachable():
    result = deepcopy(FORCING)
    result['patches'] = [{'target': 'publisher.Publisher.try_adopt', 'delay_seconds': 1}]
    return result


def test_leaf_without_rehearsal_is_refused_before_maker(rehearsal):
    _, _, inputs, _ = rehearsal
    reply = plan()
    planner.check_reply(inputs, reply)
    assert any('matching forcing rehearsal receipt' in v for v in reply['violations'])


def test_unreachable_slow_decision_reports_zero_hits_and_green_base(rehearsal):
    ledger, _, inputs, calls = rehearsal
    result = ledger.run([NODE], deepcopy(FORCING))
    assert result['status'] == 'refused'
    assert 'zero-hit' in result['message'] and 'publisher.Publisher.decide' in result['message']
    assert result['forced']['tests'][0]['target_hits'] == {'publisher.Publisher.decide': 0}
    assert result['forced']['tests'][0]['outcome'] == 'passed'
    assert ledger.violations(inputs['base_sha'], plan()['checks'])
    assert 1 <= len(calls) <= 2


def test_reached_assertion_has_bound_verified_receipts(rehearsal):
    ledger, root, inputs, calls = rehearsal
    forcing = reachable()
    # An uncommitted change must never affect the pinned rehearsal.
    (root / 'publisher.py').write_text('raise RuntimeError("dirty checkout")\n')
    result = ledger.run([NODE], forcing)
    assert result['status'] == 'validated'
    assert result['base_sha'] == inputs['base_sha'] and result['forcing'] == forcing
    assert result['selectors'] == [NODE]
    assert result['forced']['tests'][0]['target_hits'] == {'publisher.Publisher.try_adopt': 1}
    assert result['forced']['tests'][0]['outcome'] == 'failed'
    assert len(result['forced']['receipts'][0]['receipt_sha256']) == 64
    assert ledger.violations(inputs['base_sha'], plan(forcing)['checks']) == []
    assert len(calls) == 2
    assert all('--anywhere' in cmd and '--demand' in cmd and '--cpus' in cmd for cmd in calls)


def test_killed_fork_target_validates_exact_rehearsal_receipts(rehearsal):
    ledger, root, inputs, calls = rehearsal
    (root / 'tests/test_adoption.py').write_text('''
import os
import signal
import sys
import time
from publisher import Publisher

def test_adopt():
    reader, writer = os.pipe()
    child = os.fork()
    if child == 0:
        os.close(reader)
        def entering_sleep(frame, event, function):
            if event == 'c_call' and function is time.sleep:
                os.write(writer, b'E')
                sys.setprofile(None)
        sys.setprofile(entering_sleep)
        Publisher().try_adopt()
        os.write(writer, b'D')
        os._exit(0)
    os.close(writer)
    try:
        observed = os.read(reader, 1)
        if observed == b'E':
            os.kill(child, signal.SIGKILL)
    finally:
        os.close(reader)
        os.waitpid(child, 0)
    assert observed == b'D', 'child did not finish submission'
''')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'forking base')
    inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
    ledger = ForcingRehearsals(ledger.worktree, inputs)
    forcing = reachable()
    result = ledger.run([NODE], forcing)
    assert result['forced']['tests'][0]['outcome'] == 'failed'
    assert result['forced']['tests'][0]['target_hits'] == {'publisher.Publisher.try_adopt': 1}
    assert result['status'] == 'validated', result
    assert result['base_sha'] == inputs['base_sha'] and result['forcing'] == forcing
    assert result['selectors'] == [NODE]
    assert len(result['forced']['receipts'][0]['receipt_sha256']) == 64
    assert ledger.violations(inputs['base_sha'], plan(forcing)['checks']) == []
    assert len(calls) == 2


@pytest.mark.parametrize('control', ['still_green', 'unrelated', 'setup', 'import', 'missing', 'no_report'])
def test_unusable_experiments_cannot_supply_red_or_repeat_identically(rehearsal, monkeypatch, control):
    ledger, root, inputs, calls = rehearsal
    forcing = reachable()
    if control == 'still_green':
        forcing['patches'][0]['delay_seconds'] = 0.001
    elif control == 'unrelated':
        (root / 'tests/test_adoption.py').write_text(TEST + '\n    assert False, "unrelated"\n')
    elif control == 'setup':
        (root / 'tests/conftest.py').write_text('import pytest\n@pytest.fixture(autouse=True)\ndef broken():\n    raise RuntimeError("setup")\n')
    elif control == 'import':
        (root / 'tests/test_adoption.py').write_text('import absent_fixture_module\n' + TEST)
    elif control == 'missing':
        (root / 'tests/test_adoption.py').write_text(TEST.replace('def test_adopt', 'def test_other'))
    elif control == 'no_report':
        (root / 'tests/test_adoption.py').write_text('import os\nos._exit(0)\n')
    if control in {'unrelated', 'setup', 'import', 'missing', 'no_report'}:
        git(root, 'add', '.')
        git(root, 'commit', '-qm', control)
        inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
        ledger = ForcingRehearsals(ledger.worktree, inputs)
    result = ledger.run([NODE], forcing)
    expected = 'infrastructure_error' if control == 'no_report' else 'refused'
    assert result['status'] == expected
    count = len(calls)
    again = ledger.run([NODE], deepcopy(forcing))
    assert again['status'] == expected and len(calls) == count
    assert ledger.violations(inputs['base_sha'], plan(forcing)['checks'])


@pytest.mark.parametrize('change', ['base', 'selectors', 'target', 'value', 'environment', 'receipt'])
def test_changed_or_stale_evidence_is_unusable(rehearsal, change):
    ledger, _, inputs, _ = rehearsal
    forcing = reachable()
    assert ledger.run([NODE], forcing)['status'] == 'validated'
    checks = plan(forcing)['checks']
    base = inputs['base_sha']
    if change == 'base':
        base = 'f' * 40
    elif change == 'selectors':
        checks[0]['node_ids'].append('tests/test_adoption.py::test_other')
    elif change == 'target':
        checks[0]['forcing']['patches'][0]['target'] = 'publisher.Publisher.decide'
    elif change == 'value':
        checks[0]['forcing']['patches'][0]['delay_seconds'] = 2
    elif change == 'environment':
        checks[0]['forcing']['environment'] = {'ADOPTION': '0'}
    else:
        ledger.receipts.clear()
    assert ledger.violations(base, checks)


def test_planner_tool_allows_only_declarative_forcing_and_bounded_actions(rehearsal):
    ledger, _, _, calls = rehearsal
    refused = ledger.run([NODE], {**FORCING, 'command': 'touch /tmp/arbitrary'})
    assert refused['status'] == 'refused' and 'command' in refused['message']
    assert calls == []
    assert ledger.run([NODE], reachable())['status'] == 'validated'
    count = len(calls)
    assert ledger.run([NODE], reachable())['status'] == 'validated'
    assert len(calls) == count
    for index in range(10):
        forcing = reachable()
        forcing['symptom'] += str(index)
        ledger.run([NODE], forcing)
    assert len(calls) <= 16


def test_independent_checking_is_still_required(rehearsal):
    ledger, _, _, _ = rehearsal
    result = ledger.run([NODE], reachable())
    assert result['status'] == 'validated'
    assert result['pending_checking'] is True
    assert 'head' not in result and 'head_forced' not in result


@pytest.mark.parametrize('reached', [False, True])
def test_real_planner_tool_loop_refuses_or_binds_handoff(rehearsal, monkeypatch, reached):
    from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
    from langchain_core.messages import AIMessage
    from tl3_workers import runtime

    ledger, root, inputs, calls = rehearsal
    inputs['repair_plan_consistency'] = True
    forcing = reachable() if reached else deepcopy(FORCING)
    draft = plan(forcing)
    if not reached:
        # A failed experiment must bypass even format repair and #577 correction.
        draft['edits_tests'].append('tests/test_adoption.py::test_uncovered')
    final = json.dumps(draft) if reached else 'malformed final reply after failed work'
    model = FakeMessagesListChatModel(responses=[
        AIMessage(content='', tool_calls=[{'name': 'rehearse_forcing', 'id': 'force',
                                          'args': {'tests': [NODE], 'forcing': forcing}}]),
        AIMessage(content=final),
        AIMessage(content=json.dumps(plan(reachable()))),
    ])
    monkeypatch.setattr(runtime, 'create_model', lambda _: model)
    request = {'role': 'planner', 'worktree': str(root), 'inputs': inputs,
               'operation': {'node': 1, 'generation': 0, 'kind': 'planning'},
               'model': {'name': 'fixture', 'effort': 'high', 'budget_tokens': 32000}}
    events = []
    runtime.run(request, events.append)
    reply = events[-1]['reply']
    assert model.i == 2, 'failed work must never enter another model correction turn'
    assert len(calls) == 2
    if reached:
        assert reply['kind'] == 'leaf' and reply['violations'] == []
        assert reply['checks'] == draft['checks']
        assert reply['forcing_rehearsals'][0]['forcing'] == forcing
        assert reply['forcing_rehearsals'][0]['base_sha'] == ledger.base_sha
    else:
        assert reply['kind'] == 'cant_meet' and 'zero-hit' in reply['report']
        assert reply['violations'] and 'zero-hit' in reply['violations'][0]
        assert reply['forcing_rehearsals'][0]['forced']['tests'][0]['outcome'] == 'passed'



@pytest.mark.parametrize('stage', ['unforced', 'forced'])
@pytest.mark.parametrize('status,message', [
    ('not_available', 'Repository test settings are unavailable.'),
])
def test_rehearsal_failure_diagnostic_reaches_cant_meet(rehearsal, monkeypatch, stage, status, message):
    from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
    from langchain_core.messages import AIMessage
    from tl3_workers import forcing_rehearsal, runtime

    _, root, inputs, _ = rehearsal
    original = forcing_rehearsal.make_test_tool

    def test_tool(worktree, **kwargs):
        if bool(kwargs.get('forcing')) == (stage == 'forced'):
            return lambda *args, **options: {'status': status, 'message': message}
        return original(worktree, **kwargs)

    monkeypatch.setattr(forcing_rehearsal, 'make_test_tool', test_tool)
    model = FakeMessagesListChatModel(responses=[
        AIMessage(content='', tool_calls=[{'name': 'rehearse_forcing', 'id': 'force',
                                          'args': {'tests': [NODE], 'forcing': reachable()}}]),
        AIMessage(content='malformed reply after failed work'),
        AIMessage(content=json.dumps(plan(reachable()))),
    ])
    monkeypatch.setattr(runtime, 'create_model', lambda _: model)
    inputs['repair_plan_consistency'] = True
    request = {'role': 'planner', 'worktree': str(root), 'inputs': inputs,
               'operation': {'node': 1, 'generation': 0, 'kind': 'planning'},
               'model': {'name': 'fixture', 'effort': 'high', 'budget_tokens': 32000}}
    events = []
    runtime.run(request, events.append)
    reply = events[-1]['reply']
    assert reply['kind'] == 'cant_meet'
    assert model.i == 2, 'failed work must never enter another model correction turn'
    assert status in reply['report']
    assert message in reply['report']
    assert reply['violations']


def test_every_named_old_path_must_hit_the_delayed_target(rehearsal):
    ledger, root, inputs, _ = rehearsal
    (root / 'tests/test_adoption.py').write_text(TEST + '\ndef test_bypass():\n    assert Publisher().decide()\n')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'second path bypasses adoption')
    inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
    ledger = ForcingRehearsals(ledger.worktree, inputs)
    result = ledger.run([NODE, 'tests/test_adoption.py::test_bypass'], reachable())
    assert result['status'] == 'refused'
    assert 'zero-hit' in result['message'] and 'test_bypass' in result['message']


@pytest.mark.parametrize('changed', ['forcing', 'selectors'])
def test_pb_verified_log_for_another_forcing_or_selection_is_not_evidence(rehearsal, monkeypatch, changed):
    from tl3_workers import pb_tests
    ledger, _, _, _ = rehearsal
    original = pb_tests._summary

    def stale(ending, key):
        result = original(ending, key)
        if 'forcing' in result:
            result[changed] = {} if changed == 'forcing' else ['tests/test_other.py::test_other']
        return result

    monkeypatch.setattr(pb_tests, '_summary', stale)
    result = ledger.run([NODE], reachable())
    assert result['status'] == 'infrastructure_error'
    assert result['forced']['status'] == 'infrastructure_error'
    assert ledger.failure is None


@pytest.mark.parametrize("timeout", [120, 300, 600])
def test_generator_repository_deadline_reaches_inner_and_outer_budgets(
        generator_repo, generator_backend, tmp_path, timeout):
    tree, generate = configured_generator(generator_repo, tmp_path)
    tree.generator_timeout_s = timeout
    with tree:
        result = generate()
    assert result["status"] == "completed", result
    [args] = generator_backend[0]
    assert args[args.index("--timeout-s") + 1] == str(timeout + 60)
    assert args[args.index("--progress-phase") + 1] == f"tests={timeout + 60}"
    assert result.get("budget", {}).get("timeout_s") == timeout
    assert result["outer"].get("elapsed_s") == 123.5, "retain PB's whole action time separately from harness stages"


@pytest.mark.parametrize("change", ["cpus", "mem_gb", "deadline"])
def test_generator_budget_change_cannot_reuse_a_checkpoint(
        generator_repo, generator_backend, tmp_path, change):
    tree, generate = configured_generator(generator_repo, tmp_path)
    with tree:
        first = generate()
        assert first["status"] == "completed", first
        if change == "deadline":
            tree.generator_timeout_s = 300
        else:
            tree.test_settings = TestSettings(IMAGE, tmp_path / "tools",
                                             cpus=4 if change == "cpus" else 1,
                                             mem_gb=12 if change == "mem_gb" else 2)
        second = generate()
    assert second["status"] == "completed", second
    assert len(generator_backend[0]) == 2, "changed budgets must submit a fresh sealed action"
    assert first["identity"] != second["identity"]
    assert second["output_count"] == 0


@pytest.mark.parametrize("fail", [False, True])
def test_generator_trusted_phase_receipts_include_timeout_and_cleanup(
        generator_repo, generator_backend, tmp_path, monkeypatch, fail):
    import shutil
    from tl3_harness import generator
    staged = tmp_path / "staged"
    shutil.copytree(generator_repo, staged, ignore=shutil.ignore_patterns(".git"))
    if fail:
        def timeout(*args, **kwargs):
            raise subprocess.TimeoutExpired(args[0], kwargs["timeout"])
        monkeypatch.setattr(generator, "container_command", lambda *args: ["generator-fixture"])
        original = generator.subprocess.run
        def launch(args, **kwargs):
            return timeout(args, **kwargs) if args == ["generator-fixture"] else original(args, **kwargs)
        monkeypatch.setattr(generator.subprocess, "run", launch)
    result = generator.run(staged, generator.Container(IMAGE), "python3 generate.py",
                           ["**/BUILD.bazel"], [], ["tests/BUILD.bazel"])
    assert result["status"] == ("failed" if fail else "completed"), result
    assert "timings" in result, "trusted stages must survive both success and inner timeout"
    phases = result["timings"]
    for phase in ("preparation", "container", "validation", "cleanup"):
        assert phases[phase]["seconds"] >= 0
        assert phases[phase]["outcome"] in ("completed", "timeout", "skipped")
    assert phases["total_seconds"] >= sum(phases[p]["seconds"] for p in
                                              ("preparation", "container", "validation", "cleanup"))
    assert phases["container"]["scope"] == "startup_and_command"
    if fail:
        assert result["phase"] == "container"
        assert result["exit_code"] == 124
        assert phases["container"]["outcome"] == "timeout"
        assert phases["validation"]["outcome"] == "skipped"


def test_request_163_implicit_generator_failure_retains_safe_causal_receipt(
        generator_repo, generator_backend, tmp_path, monkeypatch):
    from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
    from langchain_core.messages import AIMessage
    from tl3_workers import runtime
    from tl3_workers.contracts import validate
    from tl3_harness import generator
    original = generator.subprocess.run
    def launch(args, **kwargs):
        if args == ["python3", "generate.py"]:
            raise subprocess.TimeoutExpired(args, kwargs["timeout"])
        return original(args, **kwargs)
    monkeypatch.setattr(generator.subprocess, "run", launch)
    turn = AIMessage(content=json.dumps({"kind": "done", "self_check": [
        {"goal_id": "build", "status": "met", "evidence": "Model work finished."}]}))
    monkeypatch.setattr(runtime, "create_model", lambda request: FakeMessagesListChatModel(responses=[turn]))
    request = {"protocol": 1, "role": "maker", "worktree": str(generator_repo),
               "operation": {"node": 163, "generation": 0, "kind": "make"},
               "inputs": {"brief": "Regenerate BUILD files after model work.",
                          "goals": [{"id": "build", "description": "Update BUILD files."}],
                          "plan": {"files": ["tests/BUILD.bazel"], "checks": [
                              {"name": "build", "kind": "demonstrating",
                               "tests": ["tests/test_wait.py::test_wait"], "goal_ids": ["build"]}]},
                          "forecast": {"fit_tokens": 100000, "context_tokens": 150000,
                                       "overhead_tokens": {"work": 30000},
                                       "generated_files": ["**/BUILD.bazel"],
                                       "generator": "python3 generate.py", "generator_available": True,
                                       "generator_timeout_s": 300},
                          "attempt": 1, "round": 0, "context_size": 150000,
                          "tests": {"image": IMAGE, "tools_path": str(tmp_path / "tools"),
                                    "cpus": 4, "mem_gb": 12}}}
    events = []
    runtime.run(request, events.append)
    reply = events[-1]["reply"]
    assert reply["kind"] == "cant_meet" and "head" not in reply
    assert KEY in reply["report"], "implicit failure must keep the failed action identity"
    assert "b" * 64 in reply["report"], "implicit failure must keep the verified receipt"
    assert '"exit_code": 124' in reply["report"]
    assert '"exit_code": 0' in reply["report"], "outer success must remain distinct from inner failure"
    assert '"timeout_s": 300' in reply["report"]
    assert "container" in reply["report"]
    [args] = generator_backend[0]
    assert args[args.index("--cpus") + 1] == "4"
    assert args[args.index("--demand") + 1] == "mem_gb=12"
    assert args[args.index("--timeout-s") + 1] == "360"
    assert len(generator_backend[0]) == 1, "fail_fast must not retry work"
    validate("maker-reply", reply)


@pytest.mark.parametrize("timeout", [0, -1, 601, True, "300", 1.5])
def test_generator_rejects_invalid_repository_deadline_before_submission(
        generator_repo, generator_backend, tmp_path, timeout):
    tree, generate = configured_generator(generator_repo, tmp_path)
    tree.generator_timeout_s = timeout
    with tree:
        result = generate()
    assert result["status"] == "failed", result
    assert not generator_backend[0], "invalid trusted input must fail before PB submission"


def test_failed_inner_generator_receipt_is_journaled_without_work_retry(
        generator_repo, generator_backend, tmp_path):
    (generator_repo / "generate.py").write_text("print('PRIVATE_DIAGNOSTIC')\nraise SystemExit(124)\n")
    tree, generate = configured_generator(generator_repo, tmp_path)
    with tree:
        result = generate()
        assert result["status"] == "failed", result
        assert "PRIVATE_DIAGNOSTIC" not in json.dumps(result)
        assert tree.generator_checkpoint.get("receipt") == result
        assert tree.generation_receipt == result
        assert generate() == result
    assert len(generator_backend[0]) == 1


@pytest.mark.parametrize("phase", ["preparation", "validation", "cleanup"])
def test_generator_stage_timeout_is_attributed_without_losing_inner_verdict(
        generator_repo, generator_backend, tmp_path, monkeypatch, phase):
    import shutil
    from tl3_harness import generator
    staged = tmp_path / "staged"
    shutil.copytree(generator_repo, staged, ignore=shutil.ignore_patterns(".git"))
    original = generator.manifest
    calls = 0
    def manifest(root):
        nonlocal calls
        calls += 1
        if (phase == "preparation" and calls == 1) or (phase == "validation" and calls == 2):
            raise generator.StageTimeout(phase)
        return original(root)
    monkeypatch.setattr(generator, "manifest", manifest)
    if phase == "cleanup":
        original_cleanup = generator.tempfile.TemporaryDirectory.cleanup
        def cleanup(self):
            original_cleanup(self)
            raise generator.StageTimeout("cleanup")
        monkeypatch.setattr(generator.tempfile.TemporaryDirectory, "cleanup", cleanup)
    result = generator.run(staged, generator.Container(IMAGE), "python3 generate.py",
                           ["**/BUILD.bazel"], [], ["tests/BUILD.bazel"])
    assert result["status"] == "failed", result
    assert result["phase"] == phase
    assert result["timings"][phase]["outcome"] == "timeout"
    assert result["timings"]["total_seconds"] >= result["timings"][phase]["seconds"]
    assert result["exit_code"] == (0 if phase == "cleanup" else 124)


def test_generator_still_requires_pb_admission_before_any_work(tmp_path, monkeypatch):
    from tl3_harness import generator
    for name in ("PRISMABUILD_CONTAINER_OWNER", "PRISMABUILD_CONTAINER_MARKER",
                 "PRISMABUILD_ACTION_PROGRESS_PATH", "PRISMABUILD_ACTION_PROGRESS_TOKEN"):
        monkeypatch.delenv(name, raising=False)
    with pytest.raises(RuntimeError, match="admitted PrismaBuild"):
        generator.run(tmp_path, generator.Container(IMAGE), "python3 generate.py",
                      ["**/BUILD.bazel"], [], ["tests/BUILD.bazel"])


@pytest.mark.parametrize('reason', ['timeout', 'no_progress', 'cancelled', 'lease_lost'])
def test_failure_receipts_preserve_terminal_reason_without_inventing_test_start(submission, reason):
    result = call(submission, Summary().result, ['tests/test_owned.py::test_hang'], override={
        'status': 'failed', 'succeeded': False, 'exit_code': None,
        'diagnostics': {'termination_reason': reason, 'private': 'never retain'},
    })
    assert result['status'] == 'infrastructure_error'
    assert result['receipts'][0].get('termination_reason') == reason, result
    assert result.get('execution', {}).get('outcome') == 'unknown_incomplete', result
    assert result['execution']['test_started'] is False
    assert result['selectors'] == ['tests/test_owned.py::test_hang']
    assert 'private' not in json.dumps(result)


@pytest.mark.parametrize('change', ['input', 'selectors', 'contract', 'identical'])
def test_reattach_checks_staged_input_selection_and_execution_contract(submission, monkeypatch, change):
    from tl3_workers import pb_tests
    root, scripts = submission
    (scripts / 'response.json').write_text(json.dumps(Summary().result))
    calls = []
    original = pb_tests._json_command
    with Worktree(root) as tree:
        tool = make_test_tool(tree, TestSettings(IMAGE, scripts))
        tree.pb_submitted = lambda key, base, staged: calls.append(
            {'key': key, 'tree': staged, 'identity': getattr(tree, 'pb_identity', None)})
        tool(['test_one.py::test_one'])
        saved = calls[-1]
        assert saved['identity'], 'checkpoint must bind the input and trusted executor contract'
        tree.pb_reattach = {False: saved}
        if change == 'input':
            (root / 'test_one.py').write_text('def test_one():\n    assert True\n')
        if change == 'contract':
            tool = make_test_tool(tree, TestSettings(IMAGE, scripts, quiet_seconds=60))
        def command(args):
            if args[1].endswith('pbrun.py') and change == 'identical':
                pytest.fail('identical interrupted execution must reattach')
            return original(args)
        monkeypatch.setattr(pb_tests, '_json_command', command)
        tool(['test_two.py::test_two'] if change == 'selectors' else ['test_one.py::test_one'])
    assert len(calls) == (1 if change == 'identical' else 2)
    if change != 'identical':
        assert calls[-1]['identity'] != saved['identity']


def test_owned_timeout_receipt_cannot_validate_a_q50_blocker(submission):
    from tl3_workers.tools import Evidence
    summary = Summary()
    summary.add('tests/test_owned.py::test_hang', 'error', 'Test timeout; no assertion verdict.')
    summary.result.update(execution={'outcome': 'test_timeout', 'phase': 'test_run',
                                    'termination_reason': 'no_progress', 'test_started': True},
                          selectors=['tests/test_owned.py::test_hang'])
    result = call(submission, summary.result, ['tests/test_owned.py::test_hang'])
    evidence = Evidence()
    evidence.test_result(result, (False, ('tests/test_owned.py::test_hang',)))
    assert not evidence.cites({'action_key': KEY})
    assert result['totals']['failed'] == 0 and result['totals']['error'] == 1
    assert result['receipts'][0]['receipt_sha256'] == 'b' * 64
    assert 'base' not in result, 'a hang has no assertion outcome to compare or retry'


@pytest.mark.parametrize('repeat', [1, 3])
def test_oversized_incomplete_report_cannot_become_q50_evidence(submission, monkeypatch, repeat):
    from tl3_harness import execution, test_tool
    from tl3_workers.tools import Evidence

    root, _ = submission
    names = ['test_' + str(index) + '_' + 'x' * 1780 for index in range(10)]
    selected = ['test_exit.py::' + name for name in names]
    (root / 'test_exit.py').write_text(
        'import os\nfrom pathlib import Path\n'
        "counter = Path(__file__).with_name('collections')\n"
        'count = int(counter.read_text()) + 1 if counter.exists() else 1\n'
        'counter.write_text(str(count))\n'
        f'if count == {repeat}: os._exit(0)\n'
        + ''.join(f'def {name}():\n    assert False\n' for name in names))
    monkeypatch.setattr(test_tool, 'require_admission', lambda: None)
    monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_PATH', str(root / 'progress.json'))
    monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_TOKEN', 'fixture')
    monkeypatch.setattr(test_tool, 'container_command', lambda tree, container, tests: [
        sys.executable, '-I', '-B', str(Path(execution.__file__).with_name('runner.py')),
        '-', str(tree),
    ])
    summary = test_tool.run(root, execution.Container(IMAGE), tuple(selected), repeat=repeat)
    result = call(submission, summary, selected)
    evidence = Evidence()
    evidence.test_result(result, (False, tuple(selected)))
    assert not evidence.cites({'action_key': KEY}), result
    assert result['status'] == 'infrastructure_error'
    assert result['origin'] == 'test_run'
    assert result['execution'] == {'outcome': 'unknown_incomplete', 'phase': 'unknown',
                                   'termination_reason': 'incomplete_report', 'test_started': False}
    assert result['selectors_omitted'] + len(result['selectors']) == len(selected)
    assert len(json.dumps(result).encode()) <= SUMMARY_BYTES
    assert result['receipts'][0]['receipt_sha256'] == 'b' * 64


@pytest.mark.parametrize('stage', ['admission', 'retrieval'])
def test_transport_failure_has_typed_diagnostic_and_no_test_verdict(submission, monkeypatch, stage):
    from tl3_workers import pb_tests
    original = pb_tests._json_command
    def command(args):
        if args[1].endswith('pbrun.py' if stage == 'admission' else 'pbwait.py'):
            raise OSError('transport fixture')
        return original(args)
    monkeypatch.setattr(pb_tests, '_json_command', command)
    result = call(submission, Summary().result, ['tests/test_owned.py::test_hang'])
    assert result['status'] == 'infrastructure_error'
    assert result.get('execution', {}).get('outcome') == 'transport_failure', result
    assert result['execution']['phase'] == stage
    assert result['execution']['test_started'] is False
    assert result['tests'] == [] and sum(result['totals'].values()) == 0


def test_request_2709_legacy_plan_extends_before_final_head(
        generator_repo, generator_backend, tmp_path, monkeypatch):
    from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
    from langchain_core.messages import AIMessage
    from tl3_workers import runtime
    from tl3_workers.contracts import validate
    for name in ("socket_support.py", "test_short_socket.py"):
        (generator_repo / "tests" / name).unlink()
    (generator_repo / "tests/test_wait.py").write_text("from socket import socket\n")
    done = AIMessage(content=json.dumps({"kind": "done", "self_check": [
        {"goal_id": "import", "status": "met", "evidence": "Updated test import."}]}))
    turns = [AIMessage(content="", tool_calls=[{"name": "write", "id": "edit", "args": {
        "path": "tests/test_wait.py", "content": "from pathlib import Path\n"}}]), done]
    monkeypatch.setattr(runtime, "create_model", lambda _: FakeMessagesListChatModel(responses=turns))
    request = {"protocol": 1, "role": "maker", "worktree": str(generator_repo),
        "operation": {"node": 1, "generation": 0, "kind": "make"},
        "inputs": {"brief": "Update only the test's import; never hand-edit generated files.",
            "goals": [{"id": "import", "description": "Update the test import."}],
            "plan": {"files": ["tests/test_wait.py"], "checks": [{"name": "import", "kind": "demonstrating",
                "tests": ["tests/test_wait.py::test_wait"], "goal_ids": ["import"]}]},
            "forecast": {"fit_tokens": 100000, "context_tokens": 150000, "overhead_tokens": {"work": 1},
                "generated_files": ["**/BUILD.bazel"], "generator": "python3 generate.py", "generator_available": True},
            "attempt": 1, "round": 0, "context_size": 150000,
            "tests": {"image": IMAGE, "tools_path": str(tmp_path / "tools")}}}
    events = []
    runtime.run(request, events.append)
    extension = events[-1]["reply"]
    assert extension["kind"] == "needs_files", extension
    validate("maker-reply", extension)
    assert extension["fits"] is True
    assert [entry["path"] for entry in extension["files"]] == ["tests/BUILD.bazel"]
    # The existing controller extension transition locks these files before the next make.
    request["inputs"]["plan"]["files"].extend(entry["path"] for entry in extension["files"])
    monkeypatch.setattr(runtime, "create_model", lambda _: FakeMessagesListChatModel(responses=[done]))
    runtime.run(request, events.append)
    result = events[-1]["reply"]
    assert result["kind"] == "done", result
    validate("maker-reply", result)
    tree = subprocess.check_output(["git", "rev-parse", result["head"] + "^{tree}"], cwd=generator_repo).decode().strip()
    assert result["generation_evidence"]["output_tree"] == tree
    subprocess.run(["python3", "generate.py", "--check"], cwd=generator_repo, check=True)


# #600: completed negative evidence is feedback within one bounded planning run.
@pytest.mark.parametrize('negative', ['zero_hit', 'no_red', 'setup_error'])
def test_distinct_candidate_after_negative_preserves_valid_receipts(rehearsal, monkeypatch, negative):
    ledger, _, inputs, calls = rehearsal
    first = reachable()
    assert ledger.run([NODE], first)['status'] == 'validated'
    bad = deepcopy(FORCING) if negative == 'zero_hit' else reachable()
    if negative == 'no_red':
        bad['patches'][0]['delay_seconds'] = 0.001
    elif negative == 'setup_error':
        bad['patches'] = [{'target': 'publisher.Publisher.try_adopt', 'raises': 'RuntimeError'}]
    rejected = ledger.run([NODE], bad)
    assert rejected['status'] == 'refused'
    assert ledger.failure is None, 'a completed negative candidate must not latch planning'
    assert ledger.violations(inputs['base_sha'], plan(bad)['checks'])
    third = reachable()
    third['environment'] = {'CANDIDATE': 'distinct'}
    assert ledger.run([NODE], third)['status'] == 'validated'
    assert ledger.violations(inputs['base_sha'], plan(first)['checks'] + plan(third)['checks']) == []
    count = len(calls)
    assert ledger.run([NODE], deepcopy(bad)) == rejected
    assert ledger.run([NODE], deepcopy(first))['status'] == 'validated'
    assert len(calls) == count == 6


def test_negative_cache_and_exhaustion_preserve_eight_candidate_cap(rehearsal):
    ledger, _, inputs, calls = rehearsal
    for i in range(8):
        bad = deepcopy(FORCING)
        bad['environment'] = {'CANDIDATE': str(i)}
        rejected = ledger.run([NODE], bad)
        assert rejected['status'] == 'refused'
        assert ledger.run([NODE], deepcopy(bad)) == rejected
        assert len(calls) == 2 * (i + 1), 'distinct negatives must consume their own bounded actions'
    exhausted = ledger.run([NODE], reachable())
    assert exhausted['status'] == 'exhausted'
    assert '8 rehearsals' in exhausted['message'] and '16' in exhausted['message']
    assert ledger.run([NODE], reachable()) == exhausted
    assert len(calls) == 16
    assert ledger.violations(inputs['base_sha'], plan(reachable())['checks']) == [exhausted['message']]
    assert all(cmd[cmd.index('--timeout-s') + 1] == '120' for cmd in calls)


@pytest.mark.parametrize('stage', ['unforced', 'forced'])
@pytest.mark.parametrize('boundary', ['submission', 'execution', 'retrieval', 'adapter'])
def test_rehearsal_infrastructure_exits_worker_without_reply_repair(
        rehearsal, monkeypatch, tmp_path, stage, boundary):
    from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
    from langchain_core.messages import AIMessage
    from tl3_workers import pb_tests, runtime
    _, root, inputs, calls = rehearsal
    original = pb_tests._json_command
    current_phase = None

    def command(args):
        nonlocal current_phase
        if args[1].endswith('pbrun.py'):
            current_phase = 'forced' if '--forcing' in args else 'unforced'
            if current_phase == stage and boundary == 'submission':
                raise OSError('submission fixture')
        if current_phase == stage and args[1].endswith('pbwait.py'):
            if boundary == 'retrieval':
                raise OSError('retrieval fixture')
            code, value = original(args)
            ending = value['endings'][0]
            if boundary == 'execution':
                ending.update(status='failed', succeeded=False, exit_code=137, reason='worker_lost')
            elif boundary == 'adapter':
                ending['result']['verified'] = False
            return code, value
        return original(args)

    monkeypatch.setattr(pb_tests, '_json_command', command)
    model = FakeMessagesListChatModel(responses=[
        AIMessage(content='', tool_calls=[{'name': 'rehearse_forcing', 'id': 'force',
                                          'args': {'tests': [NODE], 'forcing': reachable()}}]),
        AIMessage(content='malformed final must never be read'),
    ])
    monkeypatch.setattr(runtime, 'create_model', lambda _: model)
    facts, memory = tmp_path / 'facts.json', tmp_path / 'memory.json'
    session = tmp_path / 'planner-rehearsals.json'
    request = {'role': 'planner', 'worktree': str(root), 'inputs': inputs,
               'operation': {'node': 1, 'generation': 0, 'kind': 'planning'},
               'model': {'name': 'fixture', 'effort': 'high', 'budget_tokens': 32000},
               'trace': {'completion': str(facts)}, 'memory': {'summary_file': str(memory)},
               'session_file': str(session)}
    events = []
    with pytest.raises(Exception) as caught:
        runtime.run(request, events.append)
    error = caught.value
    assert type(error).__name__ == 'ForcingInfrastructureError'
    assert error.result['status'] == 'infrastructure_error'
    result = error.result[stage]
    assert result['execution']['phase'] == ('admission' if boundary == 'submission' else
                                           'unknown' if boundary == 'execution' else
                                           'summary' if boundary == 'adapter' else 'retrieval')
    if boundary != 'submission':
        assert result['action_key'] == KEY
    assert model.i == 1 and not any(e['event'] == 'reply' for e in events)
    assert len(calls) <= (2 if stage == 'forced' else 1)
    assert json.loads(facts.read_text())['failure']['class'] == 'ForcingInfrastructureError'
    reason = json.loads(memory.read_text())['reason']
    assert reason['failure_class'] == 'ForcingInfrastructureError'
    assert stage in reason['failure_message'] and result['execution']['phase'] in reason['failure_message']
    if boundary != 'submission':
        assert KEY in reason['failure_message']
    if boundary == 'retrieval':
        admitted = len(calls)
        monkeypatch.setattr(pb_tests, '_json_command', original)
        model.i = 0
        model.responses[1] = AIMessage(content=json.dumps(plan(reachable())))
        request['operation']['generation'] += 1
        runtime.run(request, events.append)
        assert events[-1]['reply']['kind'] == 'leaf'
        assert events[-1]['reply']['violations'] == []
        assert len(calls) == admitted + (1 if stage == 'unforced' else 0), 'recovery must reattach admitted actions'


def test_final_plan_accepts_only_matching_validated_candidates(rehearsal):
    ledger, _, inputs, _ = rehearsal
    assert ledger.run([NODE], deepcopy(FORCING))['status'] == 'refused'
    reply = plan()
    planner.check_reply(inputs, reply, rehearsals=ledger)
    assert any('matching forcing rehearsal receipt' in v for v in reply['violations'])
    good = reachable()
    assert ledger.run([NODE], good)['status'] == 'validated'
    reply = plan(good)
    planner.check_reply(inputs, reply, rehearsals=ledger)
    assert reply['violations'] == []
    assert {r['status'] for r in reply['forcing_rehearsals']} == {'refused', 'validated'}
    changed = plan(good)
    changed['checks'][0]['forcing']['environment'] = {'DIFFERENT': '1'}
    planner.check_reply(inputs, changed, rehearsals=ledger)
    assert any('matching forcing rehearsal receipt' in v for v in changed['violations'])


def test_request199_complete_summary_envelope_keeps_exact_retained_evidence(submission):
    response = Summary().result
    response.update(forcing=reachable(), selectors=[NODE])
    for i in range(24):
        response['totals']['error'] += 1
        if i < 12:
            response['tests'].append({'test': NODE + str(i), 'outcome': 'error',
                                      'message': 'resolved owner select is outside repository ' + 'x' * 950})
    response['note'] = 'Truncated: totals include all 24 setup errors.'
    response['tests'][0]['message'] += 'x' * (16058 - len(json.dumps(response).encode()))
    assert len(json.dumps(response).encode()) == 16058
    retained = deepcopy(response['tests'])
    result = call(submission, response)
    assert result['status'] == 'completed', result
    assert len(json.dumps(result).encode()) <= SUMMARY_BYTES
    assert result['totals'] == response['totals']
    assert result['forcing'] == response['forcing'] and result['selectors'] == [NODE]
    assert result['receipts'][0]['receipt_sha256'] == 'b' * 64
    assert result['tests'] and len(result['tests']) < len(retained)
    assert all(any(all(item[key] == value for key, value in original.items())
                   for original in retained) for item in result['tests'])
    assert 'Truncated' in result['note']
    from tl3_workers.forcing_rehearsal import unusable
    assert unusable(result, [NODE], 'failed') == 'incomplete node results'


def test_planner_explores_before_final_reply_without_format_repair(rehearsal, monkeypatch):
    from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
    from langchain_core.messages import AIMessage
    from tl3_workers import runtime
    _, root, inputs, calls = rehearsal
    good = reachable()
    model = FakeMessagesListChatModel(responses=[
        AIMessage(content='', tool_calls=[{'name': 'rehearse_forcing', 'id': 'negative',
                                          'args': {'tests': [NODE], 'forcing': deepcopy(FORCING)}}]),
        AIMessage(content='', tool_calls=[{'name': 'rehearse_forcing', 'id': 'distinct',
                                          'args': {'tests': [NODE], 'forcing': good}}]),
        AIMessage(content=json.dumps(plan(good))),
    ])
    monkeypatch.setattr(runtime, 'create_model', lambda _: model)
    request = {'role': 'planner', 'worktree': str(root), 'inputs': inputs,
               'operation': {'node': 1, 'generation': 0, 'kind': 'planning'},
               'model': {'name': 'fixture', 'effort': 'high', 'budget_tokens': 32000}}
    events = []
    runtime.run(request, events.append)
    reply = events[-1]['reply']
    assert reply['kind'] == 'leaf' and reply['violations'] == []
    assert len(calls) == 4
    from tl3_workers.forcing_rehearsal import GUIDANCE
    assert 'distinct' in GUIDANCE.lower() and 'infrastructure' in GUIDANCE.lower()
    assert 'failed rehearsal stops planning' not in planner.ROLE.tools['rehearse_forcing']


@pytest.mark.parametrize('module,attribute', [('time', 'monotonic'), ('select', 'select')])
def test_external_alias_owner_refused_before_admission_and_survives_stop(
        rehearsal, monkeypatch, module, attribute):
    from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
    from langchain_core.messages import AIMessage
    from tl3_workers import runtime
    ledger, root, inputs, calls = rehearsal
    (root / 'pkg').mkdir()
    (root / 'pkg/__init__.py').write_text('')
    (root / 'pkg/mod.py').write_text(
        f'import {module}\nraise RuntimeError("served imports must not run in preflight")\n')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'stdlib alias')
    inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
    ledger = ForcingRehearsals(ledger.worktree, inputs)
    target = f'pkg.mod.{module}.{attribute}'
    forcing = {'symptom': 'A global alias is unsupported.', 'patches': [{'target': target, 'returns': 1000}]}
    result = ledger.run([NODE], forcing)
    assert calls == [], 'resolved external aliases must be refused before any rehearsal action'
    assert result['status'] == 'refused'
    assert result['refusal'] == {'phase': 'forcing_validation', 'target': target,
                                  'resolved_module': module, 'owner_category': 'external_module',
                                  'reason': 'outside_served_repository'}
    assert module in result['message'] and target in result['message']
    assert ledger.run([NODE], deepcopy(forcing)) == result and calls == []
    model = FakeMessagesListChatModel(responses=[
        AIMessage(content='', tool_calls=[{'name': 'rehearse_forcing', 'id': 'alias',
                                          'args': {'tests': [NODE], 'forcing': forcing}}]),
        AIMessage(content='malformed final after unsupported capability'),
    ])
    monkeypatch.setattr(runtime, 'create_model', lambda _: model)
    request = {'role': 'planner', 'worktree': str(root), 'inputs': inputs,
               'operation': {'node': 1, 'generation': 0, 'kind': 'planning'},
               'model': {'name': 'fixture', 'effort': 'high', 'budget_tokens': 32000}}
    events = []
    runtime.run(request, events.append)
    reply = events[-1]['reply']
    assert reply['kind'] == 'cant_meet' and target in reply['report'] and module in reply['report']
    assert reply['forcing_rehearsals'][0]['refusal'] == result['refusal']
    assert calls == []


# PR #603 review: execution dedupe and final proof binding have different contracts.
@pytest.mark.parametrize('replacement', [True, 1.0])
def test_receipt_binding_and_execution_cache_preserve_json_value_types(rehearsal, replacement):
    ledger, root, inputs, calls = rehearsal
    (root / 'tests/test_adoption.py').write_text(
        'from publisher import Publisher\ndef test_adopt():\n    assert Publisher().try_adopt() is True\n')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'type-sensitive assertion')
    inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
    ledger = ForcingRehearsals(ledger.worktree, inputs)
    forcing = {'symptom': 'The return type matters.',
               'patches': [{'target': 'publisher.Publisher.try_adopt', 'returns': 1}]}
    assert ledger.run([NODE], forcing)['status'] == 'validated'
    changed = deepcopy(forcing)
    changed['patches'][0]['returns'] = replacement
    assert ledger.violations(inputs['base_sha'], plan(changed)['checks']), '1 must not bind true or 1.0'
    assert ledger.violations(inputs['base_sha'], plan(forcing)['checks']) == []
    ledger.run([NODE], changed)
    assert len(calls) == 4, 'JSON-distinct values must run as distinct experiments'


@pytest.mark.parametrize('validated', [False, True])
@pytest.mark.parametrize('cosmetic', ['symptom', 'selectors', 'patches', 'all'])
def test_execution_cache_ignores_prose_and_irrelevant_order(rehearsal, tmp_path, validated, cosmetic):
    ledger, root, inputs, calls = rehearsal
    (root / 'tests/test_adoption.py').write_text(TEST + TEST[TEST.index('def test_adopt'):].replace(
        'def test_adopt', 'def test_other'))
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'two selectors')
    inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
    checkpoint = tmp_path / 'rehearsals.json'
    ledger = ForcingRehearsals(ledger.worktree, inputs, checkpoint=checkpoint)
    forcing = reachable()
    if not validated:
        forcing['patches'][0]['delay_seconds'] = 0.001
    forcing['patches'].append({'target': 'publisher.Publisher.decide', 'returns': 1})
    selectors = [NODE, NODE.replace('::test_adopt', '::test_other')]
    first = ledger.run(selectors, forcing)
    assert first['status'] == ('validated' if validated else 'refused')
    changed = deepcopy(forcing)
    reordered = list(selectors)
    if cosmetic in {'symptom', 'all'}:
        changed['symptom'] = 'The same experiment, described differently.'
    if cosmetic in {'selectors', 'all'}:
        reordered.reverse()
    if cosmetic in {'patches', 'all'}:
        changed['patches'].reverse()
    for current in (ledger, ForcingRehearsals(ledger.worktree, inputs, checkpoint=checkpoint)):
        assert current.run(reordered, changed) == first, 'cosmetic changes must return the original evidence'
        assert current.attempts == 1 and current.actions == 2
        assert len(calls) == 2
        final = plan(changed)['checks']
        final[0]['node_ids'] = reordered
        assert current.violations(inputs['base_sha'], final), 'cache reuse must not mint a changed final receipt'
        exact = plan(forcing)['checks']
        exact[0]['node_ids'] = selectors
        assert bool(current.violations(inputs['base_sha'], exact)) is not validated


def test_inherited_repository_method_reaches_rehearsal(rehearsal):
    ledger, root, inputs, calls = rehearsal
    (root / 'publisher.py').write_text(
        'class Base:\n    def try_adopt(self):\n        return True\n'
        'class Publisher(Base):\n    pass\n')
    (root / 'tests/test_adoption.py').write_text(
        'from publisher import Publisher\ndef test_adopt():\n    assert Publisher().try_adopt()\n')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'inherited repository method')
    inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
    ledger = ForcingRehearsals(ledger.worktree, inputs)
    forcing = {'symptom': 'Inherited repository methods remain eligible.',
               'patches': [{'target': 'publisher.Publisher.try_adopt', 'returns': False}]}
    result = ledger.run([NODE], forcing)
    assert result['status'] == 'validated', result
    assert len(calls) == 2
    assert ledger.violations(inputs['base_sha'], plan(forcing)['checks']) == []


@pytest.mark.parametrize('binding', ['conditional_repository', 'dynamic_repository', 'dynamic_external',
                                     'class_global_repository'])
def test_uncertain_owner_uses_runtime_guard_and_preserves_typed_stop(rehearsal, monkeypatch, binding):
    from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
    from langchain_core.messages import AIMessage
    from tl3_workers import runtime
    ledger, root, inputs, calls = rehearsal
    (root / 'pkg').mkdir()
    (root / 'pkg/__init__.py').write_text('')
    source = ('class Clock:\n    @staticmethod\n    def monotonic():\n        return 1\n')
    source += {
        'conditional_repository': 'import time\nif True:\n    time = Clock\n',
        'dynamic_repository': 'time = (lambda: Clock)()\n',
        'dynamic_external': 'time = __import__("time")\n',
        # An executable class body rebinds the module global; source alone cannot settle it.
        'class_global_repository': 'import time\nclass Configure:\n    global time\n    time = Clock\n',
    }[binding]
    (root / 'pkg/mod.py').write_text(source)
    (root / 'tests/test_adoption.py').write_text(
        'from pkg.mod import time\ndef test_adopt():\n    assert time.monotonic() > 0\n')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'uncertain owner')
    inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
    ledger = ForcingRehearsals(ledger.worktree, inputs)
    target = 'pkg.mod.time.monotonic'
    forcing = {'symptom': 'The runtime guard decides uncertain ownership.',
               'patches': [{'target': target, 'returns': 0}]}
    result = ledger.run([NODE], forcing)
    assert len(calls) == 2, 'uncertain static owners must reach the admitted runtime guard'
    if binding != 'dynamic_external':
        assert result['status'] == 'validated', result
        return
    refusal = {'phase': 'forcing_validation', 'target': target, 'resolved_module': 'time',
               'owner_category': 'external_module', 'reason': 'outside_served_repository'}
    assert result['status'] == 'refused' and result['refusal'] == refusal
    assert result['forced']['tests'][0]['forcing_refusal'] == refusal
    assert result['forced']['tests'][0]['outcome'] == 'error'
    assert 'time' in result['message'] and 'outside_served_repository' in result['message']
    model = FakeMessagesListChatModel(responses=[
        AIMessage(content='', tool_calls=[{'name': 'rehearse_forcing', 'id': 'dynamic',
                                          'args': {'tests': [NODE], 'forcing': forcing}}]),
        AIMessage(content='malformed final after runtime ownership refusal'),
    ])
    monkeypatch.setattr(runtime, 'create_model', lambda _: model)
    request = {'role': 'planner', 'worktree': str(root), 'inputs': inputs,
               'operation': {'node': 1, 'generation': 0, 'kind': 'planning'},
               'model': {'name': 'fixture', 'effort': 'high', 'budget_tokens': 32000}}
    events = []
    runtime.run(request, events.append)
    reply = events[-1]['reply']
    assert reply['kind'] == 'cant_meet' and target in reply['report']
    assert 'outside_served_repository' in reply['report'] and 'time' in reply['report']
    assert reply['forcing_rehearsals'][0]['refusal'] == refusal
    assert len(calls) == 4, 'a new planner ledger runs once; the guard still refuses the global owner'


def test_declared_symptom_rehearsal_binds_declaration_and_fixed_campaign(rehearsal):
    ledger, root, inputs, calls = rehearsal
    (root / 'tests/test_adoption.py').write_text('''
import os
from concurrent.futures import Future
def test_adopt():
    future = Future()
    if not os.getenv('DECLARED_TIMEOUT'):
        future.set_result(None)
    future.result(timeout=0)
''')
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'original TimeoutError symptom')
    inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
    ledger = ForcingRehearsals(ledger.worktree, inputs)
    forcing = {'symptom': 'Future.result times out.', 'environment': {'DECLARED_TIMEOUT': '1'}, 'patches': []}
    symptom = {'node': NODE, 'exception': 'builtins.TimeoutError', 'phase': 'call', 'repeat': 2, 'parallel': 0}
    import inspect
    assert 'declared_symptom' in inspect.signature(ledger.run).parameters, 'rehearsal cannot declare exception RED'
    result = ledger.run([NODE], forcing, declared_symptom=symptom)
    assert result['status'] == 'validated', result
    checks = [{'kind': 'demonstrating', 'node_ids': [NODE], 'forcing': forcing, 'declared_symptom': symptom}]
    assert ledger.violations(inputs['base_sha'], checks) == []
    checks[0]['declared_symptom'] = {**symptom, 'repeat': 3}
    assert ledger.violations(inputs['base_sha'], checks), 'stale campaign receipt must be rejected'
    count = len(calls)
    assert ledger.run([NODE], forcing, declared_symptom=symptom) == result
    assert len(calls) == count, 'a declared campaign cannot retry until RED'


@pytest.mark.parametrize('mode', ['environment', 'patches'])
def test_t613_ac3_rehearsed_forcing_replaces_audit_helper_and_proves_real_assertion(rehearsal, monkeypatch, mode):
    from tl3_harness import execution, Forcing
    from tl3_harness.execution import Container, Progress, run_checks
    from tl3_harness.model import Check, Kind, Outcome, Verdict
    ledger, root, inputs, calls = rehearsal
    (root / 'publisher.py').write_text(
        'import os\ndef holder():\n    return os.environ.get("HOLDER_PATH", "tiny")\n')
    (root / 'pyproject.toml').write_text(
        '[tool.tl3.harness]\ncommand=["python", "-m", "pytest"]\ntest-files=["tests/*.py"]\n')
    old = '''import pytest
import publisher

def _eio():
    return {'errno': 5}

@pytest.mark.parametrize('limit', [4, 8])
def test_adopt(limit):
    pathname = publisher.holder()
    actual = pathname[:limit]
    assert actual == pathname
'''
    (root / 'tests/test_adoption.py').write_text(old)
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'parametrized old expectation')
    inputs['base_sha'] = git(root, 'rev-parse', 'HEAD')
    inputs['tool_capabilities'] = [{'tool': 'run_tests', 'executor': 'prismabuild_pytest',
        'runner_features': ['base_head'], 'artifacts': ['cas_receipt', 'terminal_verdict'],
        'methods': ['red_green', 'forced_reproduction.' + mode], 'protected_files': []}]
    chosen = {**inputs['tool_capabilities'][0], 'methods': ['helper_target_observer'],
              'requirement': 'Prove the bounded evidence expectation.', 'provenance': {'kind': 'method'}}
    from tl3_workers.evidence import preflight
    ingest = {**inputs['ingest'], 'evidence_contract': [deepcopy(chosen)]}
    preflight(inputs, ingest)
    audit = {'verdict': 'agree', 'objections': [], 'evidence_contract': [deepcopy(chosen)]}
    preflight({**inputs, 'ingest': ingest}, audit)
    inputs.update(ingest=ingest, audit_evidence_contract=audit['evidence_contract'])
    forcing = {'symptom': 'Holder pathname exceeds the bounded evidence length.',
               'environment': {'HOLDER_PATH': 'x' * 64} if mode == 'environment' else {},
               'patches': [{'target': 'publisher.holder', 'returns': 'x' * 64}] if mode == 'patches' else []}
    ledger = ForcingRehearsals(ledger.worktree, inputs)
    receipt = ledger.run([NODE], forcing)
    assert receipt['status'] == 'validated', receipt
    assert len(calls) == 2
    assert all(t['outcome'] == 'failed' for t in receipt['forced']['tests'])
    reply = {**plan(forcing), 'evidence_contract': []}
    planner.check_reply(inputs, reply, rehearsals=ledger)
    assert reply['kind'] == 'leaf', reply
    assert reply['violations'] == []
    assert reply['evidence_contract'] == []
    (root / 'tests/test_adoption.py').write_text(old.replace('assert actual == pathname',
                                                         'assert actual == pathname[:limit]'))
    git(root, 'add', '.')
    git(root, 'commit', '-qm', 'correct bounded expectation')
    head = git(root, 'rev-parse', 'HEAD')
    monkeypatch.setattr(execution, 'require_admission', lambda: None)
    monkeypatch.setenv('PRISMABUILD_CONTAINER_OWNER', str(root))
    monkeypatch.setattr(Progress, 'commit', lambda *_: None)
    runner = Path(execution.__file__).with_name('runner.py')
    monkeypatch.setattr(execution, 'container_command',
        lambda tree, container, tests: [sys.executable, '-I', '-B', str(runner), '-', str(tree)])
    check = Check('bounded evidence', Kind.DEMONSTRATING, (NODE,), Forcing.parse(forcing))
    result, = run_checks(root, inputs['base_sha'], head, (check,), container=Container(IMAGE))
    assert result.verdict == Verdict.PASS, result
    assert all(t.base == Outcome.ASSERTION and t.head == Outcome.PASSED for t in result.tests)
    assert result.forcing is not None
