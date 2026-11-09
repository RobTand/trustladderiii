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
from pb_fixtures import call, submission  # noqa: F401
from pb_fixtures import FLUX_EXECUTION, flux_settings, run_binding
from pb_fixtures import supported_fake_runtime as supported_fake_runtime
from tl3_harness.test_tool import SUMMARY_BYTES, Summary, infrastructure
from tl3_workers.tests_tool import make_test_tool
from tl3_workers.tools import Worktree

from worker_process import IMAGE
from worker_process import KEY
from worker_process import NODE
from worker_process import invoke
from worker_process import prepare_base_submission
from worker_process import reachable


def native_receipt(key=KEY):
    """The receipt flux_execution.execute builds from the verified terminal."""
    return {"backend": "flux", "wait_reference": key, "run_binding": run_binding(key),
            "receipt_sha256": "b" * 64, "ending": "passed"}


@pytest.mark.parametrize("outcome", ["passed", "failed", "error", "skipped"])
def test_fake_submission_returns_outcomes_and_uses_only_test_action(submission, supported_fake_runtime, outcome):
    summary = Summary()
    summary.add("tests/test_code.py::test_value", outcome, "fixture failure" if outcome == "failed" else "")
    result = call(submission, summary.result, ["tests/test_code.py::test_value"])
    expected = {**summary.result, "wait_reference": KEY, "receipts": [native_receipt()],
                "tree": (submission[1] / "tree.txt").read_text()}
    if outcome in ("failed", "error"):
        summary.result["tests"][0].update(classification="unverified", base_outcome="unverified")
        expected["base"] = {key: value for key, value in infrastructure(
            "The role did not supply a check base and checkout.").items() if key != "tests"}
    assert result == expected
    [prepared] = supported_fake_runtime.prepared
    document = prepared["document"]
    assert document["op"] == "prepare" and document["generator"] is False
    assert document["profile"] == FLUX_EXECUTION["profile"]
    assert document["image"] == IMAGE
    assert (document["cpus"], document["mem_gb"], document["parallel"]) == (1, 2, 0)
    assert document["quiet_seconds"] == 120
    assert document["walltime_seconds"] == 86400, "no per-call timeout was requested"
    assert document["arguments"] == ['["tests/test_code.py::test_value"]']
    assert supported_fake_runtime.submitted == [KEY] and supported_fake_runtime.attached == [KEY]


def test_whole_suite(submission, supported_fake_runtime):
    assert call(submission, Summary().result)["status"] == "completed"
    [prepared] = supported_fake_runtime.prepared
    assert prepared["document"]["arguments"] == ["[]"]


@pytest.mark.parametrize("ending", ["failed", "refused", "infra-lost", "cancelled", "unknown"])
def test_fake_infrastructure_failure_is_never_a_test_failure(submission, supported_fake_runtime, ending):
    if ending == "unknown":
        supported_fake_runtime.observation = lambda key: {"kind": "unknown"}
    else:
        supported_fake_runtime.ending = ending
    result = call(submission, Summary().result)
    assert result["status"] == "infrastructure_error"
    assert result["tests"] == [] and sum(result["totals"].values()) == 0
    assert result["wait_reference"] == KEY


def test_reported_container_failure_is_infrastructure(submission):
    result = call(submission, infrastructure("Container failed."))
    assert result["status"] == "infrastructure_error"
    assert result["message"] == "Container failed."


def test_submission_failure(submission, supported_fake_runtime, monkeypatch):
    from tl3_workers.reply_check import ReplyCheckInfrastructureError
    original = supported_fake_runtime.control

    def refuse(document):
        if document["op"] == "prepare":
            raise ReplyCheckInfrastructureError("fixture prepare refusal")
        return original(document)
    from tl3_workers import flux_execution
    monkeypatch.setattr(flux_execution, "control", refuse)
    # A refused preparation exits the worker for controller-owned recovery, never a test verdict.
    with pytest.raises(ReplyCheckInfrastructureError, match="fixture prepare refusal"):
        call(submission, Summary().result)
    assert supported_fake_runtime.submitted == [] and supported_fake_runtime.attached == []


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
def test_selectors_reject_options_and_escape_without_submission(submission, supported_fake_runtime, tests):
    with Worktree(submission[0]) as tree:
        tool = make_test_tool(tree, flux_settings())
        with pytest.raises(ValueError):
            tool(tests)
    assert not supported_fake_runtime.prepared and not supported_fake_runtime.submitted


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


@pytest.mark.parametrize("response", [b"[]", b"null", b"not json"])
def test_malformed_flux_payload_is_infrastructure(submission, supported_fake_runtime, response):
    def malform(payload):
        path = Path(payload["summary"]["path"])
        path.write_bytes(response)
        payload["summary"].update(bytes=len(response), sha256=hashlib.sha256(response).hexdigest())
    supported_fake_runtime.on_payload = malform
    result = call(submission, Summary().result)
    assert result["status"] == "infrastructure_error"
    assert sum(result["totals"].values()) == 0


@pytest.mark.durability
def test_wait_is_declared_before_blocking_and_on_reattach(submission, supported_fake_runtime):
    root, state = submission
    (state / "response.json").write_text(json.dumps(Summary().result))
    events = []
    observations = []

    def observe(key):
        assert events[-1] == {"event": "progress", "unit": "external_wait",
                              "wait": {"upstream": "flux", "key": KEY}}
        observations.append(key)
        if len(observations) == 1:
            return {"kind": "unknown"}
        return {"kind": "terminal", "binding": run_binding(key),
                "terminal": {"ending": "passed", "reason": "exited", "receipt": {"sha256": "b" * 64}}}
    supported_fake_runtime.observation = observe
    with Worktree(root) as tree:
        tree.progress_emit = events.append
        tool = make_test_tool(tree, flux_settings())
        first = tool()
        assert first["status"] == "infrastructure_error" and first["wait_reference"] == KEY
        result = tool()
    assert result["status"] == "completed"
    assert observations == [KEY, KEY]
    assert supported_fake_runtime.submitted == [KEY], "the retained binding is reattached, never resubmitted"
    assert supported_fake_runtime.resumed == [KEY]
    assert len(events) == 2
    assert events[0] == events[1]


def test_maker_staged_submission_retains_checkout_parent(submission, supported_fake_runtime):
    root, state = submission

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
    job = supported_fake_runtime.job

    def record(staged, document):
        (state / "parent").write_text(subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=staged, text=True).strip())
        (state / "shallow").write_text(subprocess.check_output(
            ["git", "rev-parse", "--is-shallow-repository"], cwd=staged, text=True).strip())
        return job(staged, document)
    supported_fake_runtime.job = record
    (state / "response.json").write_text(json.dumps(Summary().result))
    with Worktree(root) as tree:
        tree.test_checkout = root
        result = make_test_tool(tree, flux_settings())()
    assert result["status"] == "completed"
    assert (state / "parent").read_text() == parent, "the run must belong to the maker checkout"
    assert (state / "shallow").read_text() == "false", "the snapshot needs complete history"
    assert git("rev-parse", "HEAD") == parent
    assert git("status", "--porcelain") == before
    assert (root / "uncommitted.py").read_bytes() == b"current dirty bytes\x00\xff"


def test_staging_git_uses_closed_protocol_policy(submission, monkeypatch, tmp_path):
    from tl3_workers import tests_tool
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
    monkeypatch.setattr(tests_tool.subprocess, "run", run)
    staged = tmp_path / "input"
    staged.mkdir()
    with Worktree(submission[0]) as tree:
        tests_tool._stage(tree, staged)
    assert policies and all(policy == b"never" for policy in policies), policies


@pytest.mark.parametrize("mode", [0o755, 0o664, 0o600, 0o700, 0o777, 0o750, 0o4750, 0o610])
def test_staging_modes_match_commit_export(submission, tmp_path, mode):
    from tl3_harness.trees import export
    from tl3_workers.tests_tool import _stage

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


def test_default_submission_keeps_its_action_identity(submission, supported_fake_runtime):
    summary = Summary()
    summary.add("tests/test_code.py::test_value", "passed")
    invoke(submission, summary.result, {"tests": ["tests/test_code.py::test_value"]})
    [prepared] = supported_fake_runtime.prepared
    assert prepared["document"]["arguments"] == ['["tests/test_code.py::test_value"]']
    assert prepared["document"]["parallel"] == 0


def test_repeat_and_parallel_reach_the_action_and_reservation(submission, supported_fake_runtime):
    # #408: five fresh runs and one xdist run.
    summary = Summary()
    summary.add("tests/test_code.py::test_value", "passed")
    invoke(submission, summary.result, {"tests": ["tests/test_code.py::test_value"],
                                        "repeat": 5, "parallel": 2, "priority": -10})
    [prepared] = supported_fake_runtime.prepared
    document = prepared["document"]
    assert document["parallel"] == 2, "the backend reserves a core per xdist worker"
    assert (document["cpus"], document["mem_gb"]) == (1, 2)
    action = document["arguments"]
    assert action[action.index("--repeat") + 1] == "5"
    assert action[action.index("--parallel") + 1] == "2"
    assert document["walltime_seconds"] == 86400, "repeat stays inside the backend's progress contract"


@pytest.mark.parametrize("arguments", [
    {"repeat": 0}, {"repeat": 11}, {"repeat": True}, {"parallel": 1}, {"parallel": 5},
    {"priority": 1}, {"priority": -11},
])
def test_options_are_bounded(submission, supported_fake_runtime, arguments):
    with Worktree(submission[0]) as tree:
        tool = make_test_tool(tree, flux_settings())
        with pytest.raises(ValueError, match="repeat|parallel|priority"):
            tool(**arguments)
    assert not supported_fake_runtime.prepared and not supported_fake_runtime.submitted


def test_results_carry_receipts(submission):
    summary = Summary()
    summary.add("tests/test_code.py::test_value", "passed")
    result, tree = invoke(submission, summary.result, {"tests": ["tests/test_code.py::test_value"],
                                                      "repeat": 2})
    assert result["receipts"] == [native_receipt()]
    # The maker cites them and the reviewer sees them, bound to the tree the backend ran (#432).
    evidence = tree.evidence.test_evidence()
    staged = (submission[1] / "tree.txt").read_text()
    assert re.fullmatch("[0-9a-f]{40}|[0-9a-f]{64}", staged)
    assert evidence == [{"tests": ["tests/test_code.py::test_value"], "base": False,
                         "repeat": 2, "parallel": 0, "status": "completed", "tree": staged,
                         "totals": summary.result["totals"], "receipts": result["receipts"]}]
    restored = type(tree.evidence)()
    restored.restore(tree.evidence.state())
    assert restored.test_evidence() == evidence


def test_repeated_runs_are_never_served_from_the_cache(submission, supported_fake_runtime):
    # #432: each run_tests call is its own run with its own binding; nothing replays a stored result.
    summary = Summary()
    summary.add("tests/test_code.py::test_value", "passed")
    results = [invoke(submission, summary.result, {"tests": ["tests/test_code.py::test_value"], "repeat": 2})[0]
               for _ in range(2)]
    keys = [item["key"] for item in supported_fake_runtime.prepared]
    assert len(keys) == 2 and keys[0] != keys[1]
    assert [result["wait_reference"] for result in results] == keys
    assert supported_fake_runtime.submitted == keys


@pytest.mark.parametrize("parallel", [0, 2])
def test_configured_demand_reaches_maker_head_and_base_actions(submission, supported_fake_runtime, parallel):
    root, scripts = submission
    base, _, _, _ = prepare_base_submission(submission, head_value="broken")
    with Worktree(root) as tree:
        tree.test_settings = flux_settings(cpus=4, mem_gb=12)
        tree.check_base_sha = base
        tree.test_checkout = root
        result = make_test_tool(tree)(["test_code.py::test_value"], parallel=parallel)
    assert result["status"] == "completed"
    documents = [item["document"] for item in supported_fake_runtime.prepared]
    assert len(documents) == 2, "head failure automatically submits the base"
    for index, document in enumerate(documents):
        assert (document["cpus"], document["mem_gb"]) == (4, 12)
        assert document["parallel"] == (parallel if index == 0 else 0)


@pytest.mark.parametrize('ending', ['failed', 'refused', 'infra-lost', 'cancelled'])
def test_failed_runs_keep_their_binding_without_inventing_test_start(submission, supported_fake_runtime, ending):
    supported_fake_runtime.ending = ending
    result = call(submission, Summary().result, ['tests/test_owned.py::test_hang'])
    assert result['status'] == 'infrastructure_error'
    assert result['wait_reference'] == KEY
    assert result.get('execution', {}).get('outcome') == 'transport_failure', result
    assert result['execution']['phase'] == 'retrieval'
    assert result['execution']['test_started'] is False
    assert result['selectors'] == ['tests/test_owned.py::test_hang']
    assert 'receipts' not in result, 'a run without a verified completed report has no test receipt'


@pytest.mark.parametrize('change', ['input', 'selectors', 'contract', 'identical'])
@pytest.mark.durability
def test_reattach_checks_staged_input_selection_and_execution_contract(submission, supported_fake_runtime, change):
    root, scripts = submission
    (scripts / 'response.json').write_text(json.dumps(Summary().result))
    calls = []
    with Worktree(root) as tree:
        tool = make_test_tool(tree, flux_settings())
        tree.pb_submitted = lambda key, base, staged: calls.append(
            {'key': key, 'tree': staged, 'identity': getattr(tree, 'pb_identity', None), 'base': base})
        tool(['test_one.py::test_one'])
        saved = calls[-1]
        assert saved['identity'], 'checkpoint must bind the input and trusted executor contract'
        tree.pb_reattach = {False: {**saved, 'backend': 'flux'}}
        if change == 'input':
            (root / 'test_one.py').write_text('def test_one():\n    assert True\n')
        if change == 'contract':
            tool = make_test_tool(tree, flux_settings(quiet_seconds=60))
        result = tool(['test_two.py::test_two'] if change == 'selectors' else ['test_one.py::test_one'])
    assert len(calls) == 1, 'a retained binding is never replaced by a new submission'
    if change == 'identical':
        assert result['status'] == 'completed'
        assert supported_fake_runtime.resumed == [saved['key']]
    else:
        assert result['status'] == 'infrastructure_error'
        assert result['execution']['termination_reason'] == 'checkpoint_identity_changed'
        assert result['wait_reference'] == saved['key']
        assert supported_fake_runtime.resumed == []


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
    assert not evidence.cites({'wait_reference': KEY})
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
    monkeypatch.setattr(test_tool, 'container_command', lambda tree, container, tests: [
        sys.executable, '-I', '-B', str(Path(execution.__file__).with_name('runner.py')),
        '-', str(tree),
    ])
    summary = test_tool.run(root, execution.Container(IMAGE), tuple(selected), repeat=repeat)
    result = call(submission, summary, selected)
    evidence = Evidence()
    evidence.test_result(result, (False, tuple(selected)))
    assert not evidence.cites({'wait_reference': KEY}), result
    assert result['status'] == 'infrastructure_error'
    assert result['origin'] == 'test_run'
    assert result['execution'] == {'outcome': 'unknown_incomplete', 'phase': 'unknown',
                                   'termination_reason': 'incomplete_report', 'test_started': False}
    from tl3_harness.selector_identity import selector_identity
    assert result['selectors'] == selector_identity(selected)
    assert len(json.dumps(result).encode()) <= SUMMARY_BYTES
    assert result['receipts'][0]['receipt_sha256'] == 'b' * 64


@pytest.mark.parametrize('stage', ['admission', 'retrieval'])
def test_transport_failure_has_typed_diagnostic_and_no_test_verdict(submission, supported_fake_runtime, monkeypatch, stage):
    if stage == 'admission':
        original = supported_fake_runtime.control

        def control(document):
            if document['op'] == 'activation':
                raise OSError('transport fixture')
            return original(document)
        from tl3_workers import flux_execution
        monkeypatch.setattr(flux_execution, 'control', control)
    else:
        def observe(key):
            raise OSError('transport fixture')
        supported_fake_runtime.observation = observe
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


# #826 review: redaction must see the label before any capture/display boundary.
@pytest.mark.parametrize('boundary', ['line', 'total', 'raw'])
def test_stderr_capture_redacts_before_clipping_and_trace(boundary, tmp_path, monkeypatch, capsys):
    from tl3_harness import supervision, test_tool
    from tl3_workers.tracing import Redactor
    secret = 'zE9!pR4#cV7$' if boundary != 'raw' else 'v' * 128
    payloads = {
        'line': 'password: ' + secret + ' ' + '.' * 499,
        'total': 'password: ' + secret + '\n' + '.' * 505 + '\n' + '.' * 504,
        'raw': 'password: ' + 'v' * 40000 + '\ncontainer shim refused\n',
    }
    if boundary != 'raw':
        assert secret not in supervision._redactor.window(payloads[boundary], 0, len(payloads[boundary]))
    path = tmp_path / 'stderr.bin'
    path.write_text(payloads[boundary])
    monkeypatch.setattr(supervision, '_stderr_log_bytes', 0)
    monkeypatch.setattr(test_tool, 'require_admission', lambda: None)
    monkeypatch.setattr(test_tool, 'container_command', lambda *args: [sys.executable, '-I', '-B', '-c',
        f'import os; os.write(2, open({str(path)!r}, "rb").read()); os._exit(125)'])
    result = test_tool.run_current(tmp_path, execution_container(), ())
    records = [json.loads(line.removeprefix(supervision.FAILURE_PREFIX))
               for line in capsys.readouterr().out.splitlines() if line.startswith(supervision.FAILURE_PREFIX)]
    assert records, 'operator diagnostic was lost'
    from tl3_workers.contracts import validate
    for record in records:
        validate('execution-failure', record)
    assert secret not in json.dumps(records), 'capture clipping erased the credential label'
    assert secret not in Redactor(None).text(json.dumps(records)), 'trace sees an unlabeled secret'
    assert 'stderr_tail' not in json.dumps(result), 'operator stderr reached a model-facing summary'


def execution_container():
    from tl3_harness.execution import Container
    return Container(IMAGE)


# #826 review: both action-log serializations must exclude unredacted stderr.
def test_both_stderr_log_prefixes_are_redacted(tmp_path, monkeypatch, capsys):
    from tl3_harness import supervision, test_tool
    monkeypatch.setattr(supervision, '_stderr_log_bytes', 0)
    failure = supervision.ExecutionFailure(supervision.ExecutionOutcome.SETUP_FAILURE,
        'container_start', 'incomplete_report', stderr_tail='password: zE9!pR4#cV7$')
    failure.log()
    result = infrastructure(failure.message())
    result['execution'] = {**failure.as_json(), 'stderr_tail': failure.stderr_tail}
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(sys, 'argv', ['test_tool', IMAGE, '[]'])
    monkeypatch.setattr(test_tool, 'run', lambda *args, **kwargs: result)
    test_tool.main()
    output = capsys.readouterr().out
    assert 'TL3_EXECUTION_FAILURE ' in output and 'TL3_TEST_SUMMARY ' in output
    for prefix in (supervision.FAILURE_PREFIX, test_tool.PREFIX):
        records = [line for line in output.splitlines() if line.startswith(prefix)]
        assert records
        assert all('zE9!pR4#cV7$' not in line for line in records)


# #826 review: the real StructuredTool boundary excludes diagnostics in every shape.
@pytest.mark.parametrize('mode', ['plain', 'base', 'repeat', 'nested'])
def test_run_tests_tool_never_returns_operator_stderr(submission, mode):
    result = infrastructure('Container failed.')
    result['execution'] = {'outcome': 'setup_failure', 'phase': 'container_start',
        'termination_reason': 'incomplete_report', 'test_started': False,
        'stderr_tail': 'operator-only instruction from stderr'}
    if mode == 'nested':
        nested = deepcopy(result)
        result.update(groups=[nested], runs=[nested], base=nested)
    args = {'tests': ['test_code.py::test_value']}
    if mode == 'base':
        prepare_base_submission(submission)
        args['base'] = True
    if mode == 'repeat':
        args['repeat'] = 2
    observed, _ = invoke(submission, result, args)
    assert 'stderr_tail' not in json.dumps(observed)
    assert 'operator-only instruction' not in json.dumps(observed)


@pytest.mark.parametrize('context_kind', ['ini', 'csv', 'private-key'])
def test_stderr_raw_boundary_keeps_section_context_before_redaction(context_kind, monkeypatch, capsys):
    """#826: dropping an incomplete first line must not erase a retained credential section."""
    from tl3_harness.supervision import Supervised, ExecutionOutcome, MAX_STDERR_BYTES
    from tl3_workers.tracing import Redactor
    from tl3_harness import supervision
    monkeypatch.setattr(supervision, '_stderr_log_bytes', 0)
    secret = 'k' * 64 if context_kind == 'private-key' else 'zE9!pR4#cV7$'
    if context_kind == 'ini':
        header = b'[passwords]\n'
        first = header + b'.' * (MAX_STDERR_BYTES - len(header))
        value = 'value = ' + secret
    elif context_kind == 'csv':
        header = b'name,password\n'
        first = header + b'.' * (MAX_STDERR_BYTES - len(header) - 2) + b',x'
        value = 'account,' + secret
    else:
        header = b'-----BEGIN PRIVATE KEY-----\n'
        first = (header + (b'A' * 64 + b'\n') * 256)[:MAX_STDERR_BYTES]
        value = secret + '\n-----END PRIVATE KEY-----'
    later = ('\n' + value + '\ncontainer shim refused\n').encode()
    assert secret not in supervision._redactor.window((first + later).decode(), 0, len(first + later))
    # Both the exact review sequence and the earlier incomplete-boundary regression.
    for prefix in (b'', b'x' * MAX_STDERR_BYTES):
        process = Supervised([], None, ())
        for chunk in (prefix, first, later, b'\n'):
            if chunk:
                assert len(chunk) <= MAX_STDERR_BYTES
                process.capture_stderr(chunk)
        failure = process.failure(ExecutionOutcome.SETUP_FAILURE, 'container_start', 'incomplete_report')
        diagnostic = failure.operator_json()
        failure.log()
        output = capsys.readouterr().out
        assert 'TL3_EXECUTION_FAILURE ' in output
        assert secret not in output, 'later raw cut re-exposed a redacted secret in the operator log'
        assert secret not in json.dumps(diagnostic), 'later raw cut replaced redaction with a secret'
        assert secret not in Redactor(None).text(json.dumps(diagnostic))
        assert diagnostic['stderr_tail'] == 'stderr withheld: redaction context lost'


@pytest.mark.parametrize('context_kind', ['ini', 'csv', 'private-key'])
def test_new_stderr_secret_after_context_eviction(context_kind, monkeypatch, capsys):
    """#826 review3: new bytes remain secret after their governing header is evicted."""
    from tl3_harness import supervision
    from tl3_harness.supervision import Supervised, ExecutionOutcome, MAX_STDERR_BYTES
    from tl3_workers.tracing import Redactor
    monkeypatch.setattr(supervision, '_stderr_log_bytes', 0)
    secret = 'k' * 64 if context_kind == 'private-key' else 'zE9!pR4#cV7$'
    if context_kind == 'ini':
        header = b'[passwords]\n'
        first = header + b'.' * (MAX_STDERR_BYTES - len(header))
        advance = b'\n'
        value = 'value = ' + secret
    elif context_kind == 'csv':
        header = b'name,password\n'
        first = header + b'.' * (MAX_STDERR_BYTES - len(header) - 2) + b',x'
        advance = b'\nother,x'
        value = 'account,' + secret
    else:
        header = b'-----BEGIN PRIVATE KEY-----\n'
        first = (header + (b'A' * 64 + b'\n') * 256)[:MAX_STDERR_BYTES]
        advance = b'\n' + b'A' * 64 + b'\n'
        value = secret + '\n-----END PRIVATE KEY-----'
    chunks = (first, advance, ('\n' + value + '\ncontainer shim refused\n').encode(), b'\n')
    whole = b''.join(chunks).decode()
    assert secret not in supervision._redactor.window(whole, 0, len(whole))
    process = Supervised([], None, ())
    for chunk in chunks:
        assert len(chunk) <= MAX_STDERR_BYTES
        process.capture_stderr(chunk)
    failure = process.failure(ExecutionOutcome.SETUP_FAILURE, 'container_start', 'incomplete_report')
    diagnostic = failure.operator_json()
    failure.log()
    output = capsys.readouterr().out
    assert 'TL3_EXECUTION_FAILURE ' in output
    assert secret not in output, 'new secret after context eviction leaked into the PB log'
    assert secret not in json.dumps(diagnostic), 'operator JSON leaked a new secret after context eviction'
    assert secret not in Redactor(None).text(json.dumps(diagnostic))
    assert diagnostic['stderr_tail'] == 'stderr withheld: redaction context lost'


@pytest.mark.parametrize('role', ['planner', 'maker'])
def test_proof_refusal_keeps_shared_rule_data(submission, monkeypatch, role):
    from tl3_harness.model import ProofRefusal
    from tl3_workers import tests_tool
    root, scripts = submission
    def refused(*args, **kwargs):
        assert kwargs['role'] == role
        raise ProofRefusal('proof.observer.module_missing', module='tests/test_missing.py', role=role, field='/checks/0/tests/0')
    monkeypatch.setattr(tests_tool, '_stage', refused)
    with Worktree(root) as tree:
        result = make_test_tool(tree, flux_settings(), role=role)(['tests/test_missing.py::test_value'])
    data = result['proof_refusal']
    assert result['status'] == 'not_available'
    assert data['rule'] == 'proof.observer.module_missing'
    assert data['violations'][0]['id'] == data['rule']
    assert data['violations'][0]['gating'] is True
    assert {'kind': 'field', 'value': '/checks/0/tests/0'} in data['violations'][0]['names']
    assert data['reason'] in result['message']
