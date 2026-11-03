# ruff: noqa: F811

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
from tl3_harness.test_tool import SUMMARY_BYTES, Summary, infrastructure
from tl3_workers.pb_tests import TestSettings, make_test_tool
from tl3_workers.tools import Worktree

from worker_process import IMAGE
from worker_process import KEY
from worker_process import NODE
from worker_process import call
from worker_process import invoke
from worker_process import prepare_base_submission
from worker_process import reachable
from worker_process import submission  # noqa: F401
from worker_process import submitted


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
    from tl3_harness.scratch import environment as scratch_environment
    expected_env = {**scratch_environment(), 'OMP_NUM_THREADS': '1', 'MKL_NUM_THREADS': '1', 'OPENBLAS_NUM_THREADS': '1'}
    assert dict(item.split('=', 1) for item in environment) == expected_env
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
