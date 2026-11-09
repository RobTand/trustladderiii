"""The run_tests factory. The role supplies the base and execution settings."""
from dataclasses import dataclass
from functools import cache
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess

from tl3_harness import execution
from tl3_harness.proof_rules import STAGED_CHECK_COUNT
from tl3_harness.model import Check, Kind, Proof, ProofRefusal, Forcing, ALWAYS_CHECK
from tl3_harness.proof import assemble_proof
from tl3_harness.scratch import environment as scratch_environment
from tl3_harness.node_ids import selects
from tl3_harness.supervision import ExecutionFailure, ExecutionOutcome, model_result
from tl3_harness.test_tool import SUMMARY_BYTES, OUTCOMES, infrastructure, options
from tl3_harness.trees import export, configuration
from tl3_workers import reply_check, flux_execution
from tl3_workers.reply_check import ReplyCheckInfrastructureError
import jsonschema


@cache
def _harness_validator(kind):
    source = Path(__file__).with_name('schemas') / 'harness-wire.json'
    return jsonschema.Draft202012Validator(json.loads(source.read_text())['responses'][kind])


def _harness_check(command, kind, **facts):
    document = reply_check.run_check(['check-harness', command], {'kind': kind, **facts}, name='harness')
    try:
        if not isinstance(document, dict) or set(document) != {'result'}:
            raise ValueError()
        _harness_validator(kind).validate(document['result'])
    except (ValueError, KeyError, TypeError, OSError, jsonschema.ValidationError, jsonschema.SchemaError):
        raise ReplyCheckInfrastructureError('harness validator returned invalid output') from None
    return document['result']


def bounded(result, *, limit=SUMMARY_BYTES - 2048):
    response = _harness_check('summary', 'bounded', summary_json=json.dumps(result, allow_nan=False), limit=limit)
    try:
        return json.loads(response['summary_json'])
    except (ValueError, TypeError):
        raise ReplyCheckInfrastructureError('harness validator returned invalid output') from None


def selector_identity(selectors):
    selected = [list(map(ord, selector)) for selector in selectors]
    response = _harness_check('identity', 'identity', selectors=selected)
    try:
        # Preserve the legacy JSON field order at this transport boundary.
        return {'schema': response['schema'], 'count': response['count'],
                'sha256': response['sha256'],
                'prefix': [''.join(map(chr, points)) for points in response['prefix']]}
    except (ValueError, TypeError):
        raise ReplyCheckInfrastructureError('harness validator returned invalid output') from None


def _candidate_fact(value):
    """Serialize Python strings explicitly; JSON merges adjacent surrogate escapes."""
    if value is None:
        return {'kind': 'null'}
    if isinstance(value, bool):
        return {'kind': 'boolean', 'value': value}
    if isinstance(value, (int, float)):
        return {'kind': 'number', 'json': json.dumps(value)}
    if isinstance(value, str):
        return {'kind': 'text', 'codepoints': list(map(ord, value))}
    if isinstance(value, list):
        return {'kind': 'array', 'items': [_candidate_fact(item) for item in value]}
    if isinstance(value, dict) and all(isinstance(key, str) for key in value):
        return {'kind': 'object', 'fields': [[key, _candidate_fact(item)] for key, item in value.items()]}
    return {'kind': 'invalid'}


def selection_matches(value, selectors):
    selected = [list(map(ord, selector)) for selector in selectors]
    return _harness_check('identity', 'matches', selectors=selected, candidate=_candidate_fact(value))


@dataclass(frozen=True)
class TestSettings:
    image: str
    cpus: int = 1
    mem_gb: int = 2
    quiet_seconds: int = 120
    execution: dict | None = None

    @property
    def backend(self):
        return self.execution['backend']

    def __post_init__(self):
        if not re.fullmatch(r"(?:[\w./:-]+@)?sha256:[0-9a-f]{64}", self.image):
            raise ValueError("Use an immutable container image digest")
        if (not isinstance(self.execution, dict) or set(self.execution) != {'backend', 'profile'}
                or self.execution['backend'] != 'flux' or not isinstance(self.execution['profile'], str)
                or not Path(self.execution['profile']).is_absolute()):
            raise ValueError("Use the controller's selected execution profile")
        if any(type(v) is not int or v < 1 for v in (self.cpus, self.mem_gb, self.quiet_seconds)):
            raise ValueError("Test resource and quiet allowances must be positive integers")


# Avoid pytest treating this settings type as a test class.
TestSettings.__test__ = False


def _selectors(tests):
    if not isinstance(tests, list) or len(tests) > 256:
        raise ValueError("Use a list of at most 256 test selectors")
    for test in tests:
        if (not isinstance(test, str) or not test or len(test) > 2048
                or any(c in test for c in ("\x00", "\\", "\n", "\r"))):
            raise ValueError("Use relative pytest paths or node IDs")
        path = test.split("::", 1)[0]
        if (not path or path.startswith(("/", "-"))
                or any(part in ("..", ".git") for part in PurePosixPath(path).parts)):
            raise ValueError("Use relative pytest paths or node IDs")
    return tuple(dict.fromkeys(tests))


def _priority(priority):
    # TL3's checks run at -10; requests stay at or below the default.
    if priority is not None and (type(priority) is not int or not -10 <= priority <= 0):
        raise ValueError("Use priority -10 to 0, or leave it out for the default")
    return priority


def _stage(worktree, root, base_sha=None, *, action="test_tool", checks=(), forcing=None, declared_symptom=None, selected=(), role='maker', check_index=0):
    harness = Path(execution.__file__).resolve().parent
    owner = os.fstat(worktree._root)
    if any(os.path.samestat(parent.stat(), owner) for parent in (harness, *harness.parents)):
        raise ValueError("Install the trusted harness outside the role worktree")
    checkout = root / "checkout"
    head = root / 'head-source' if base_sha is not None else checkout
    head.mkdir()
    with worktree._directory(()) as directory:
        for name, is_dir in worktree._walk(directory):
            target = head / name
            if is_dir:
                target.mkdir()
            else:
                with worktree._file(name) as source, target.open('wb') as destination:
                    shutil.copyfileobj(source.buffer, destination)
                    mode = os.fstat(source.fileno()).st_mode
                    os.fchmod(destination.fileno(), 0o755 if mode & 0o100 else 0o644)
    if base_sha is not None:
        base = root / 'base-source'
        export(worktree.test_checkout, base_sha, base)
        proving = [check for check in checks if check['kind'] in ('demonstrating', 'reproducing')]
        if len(proving) > STAGED_CHECK_COUNT:
            raise ProofRefusal('proof.staging.single_check', role=role, field='/checks',
                               reply={'checks': list(checks)}, bad_value=len(proving))
        item = proving[0] if proving else {}
        proof = Proof.parse(item['proof']) if item.get('proof') else None
        tests = tuple(item.get('tests', ()))
        projection_forcing = forcing or (Forcing.parse(item['forcing'], check_index=check_index, role=role) if item.get('forcing') else None)
        projection_symptom = declared_symptom or item.get('declared_symptom')
        check = Check(item.get('name', 'development proof') if proving or forcing or declared_symptom else ALWAYS_CHECK,
                      Kind(item.get('kind', 'demonstrating')) if proving or forcing or declared_symptom else Kind.REGRESSION,
                      tests or selected or ('_tl3_unselected.py::test_outcome',), projection_forcing, proof, projection_symptom)
        receipt = assemble_proof(base, head if proving else base, checkout, check, configuration(base) if proving else {}, role=role, check_index=check_index, footprint=getattr(worktree, 'declared_files', ()) or (),
                                 protected=getattr(worktree, 'protected_paths', ()), generated=getattr(worktree, 'generated_files', ()))
        (root / 'proof.json').write_text(json.dumps(receipt, sort_keys=True))
        shutil.rmtree(base)
        shutil.rmtree(head)
    trusted = root / "trusted"
    trusted.mkdir()
    shutil.copytree(harness, trusted / "tl3_harness",
                    ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
    (trusted / "worker-contract.sha256").write_text(hashlib.sha256(Path(__file__).read_bytes()).hexdigest())
    (root / "action.py").write_text(
        'import sys\nfrom pathlib import Path\n'
        'sys.path.insert(0, str(Path(__file__).resolve().parent / "trusted"))\n'
        f'from tl3_harness.{action} import main\nmain()\n'
    )
    # The run backend snapshots a Git checkout. These fixed commands create its private input,
    # never invoking hooks or commands from the role's Git configuration.
    commands = [("init", "--quiet", "--template=")]
    maker_checkout = getattr(worktree, "test_checkout", None)
    if maker_checkout is not None:
        # The snapshot parent proves this run belongs to the maker's checkout.
        # Keep the private harness input dirty so the snapshot seals it above that parent.
        commands.extend([
            ("fetch", "--no-tags", "--no-recurse-submodules", "--", str(maker_checkout), "HEAD"),
            ("update-ref", "HEAD", "FETCH_HEAD"),
        ])
    commands.append(("add", "--force", "--all"))
    if maker_checkout is None:
        commands.append(("commit", "--quiet", "-m", "Freeze test input"))
    for args in commands:
        _git(root, *args)
    # The tested tree: the maker binds evidence to a head only when its tree matches.
    return _git(root, "write-tree", "--prefix=checkout/").strip()


def _git(root, *args):
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
               GIT_TERMINAL_PROMPT="0", GIT_NO_LAZY_FETCH="1",
               GIT_EDITOR="true", GIT_SEQUENCE_EDITOR="true",
               GIT_AUTHOR_NAME="TL3", GIT_AUTHOR_EMAIL="tl3@example.invalid",
               GIT_COMMITTER_NAME="TL3", GIT_COMMITTER_EMAIL="tl3@example.invalid")
    return subprocess.run(["git", "--no-replace-objects", "-c", "core.hooksPath=/dev/null",
                          "-c", "credential.helper=", "-c", "protocol.allow=never",
                          "-c", "protocol.file.allow=always", "-c", "core.fsmonitor=false",
                          "-c", "commit.gpgsign=false", *args],
                         cwd=root, env=env, check=True, text=True,
                         stdout=subprocess.PIPE, stderr=subprocess.DEVNULL).stdout


def _verified_test_summary(result, run_records):
    if not isinstance(result, dict):
        raise ValueError("Invalid summary structure")
    if result.get('declared_symptom') is not None:
        summaries = result.get('symptom_runs', [])
        if len(run_records) != len(summaries):
            raise ValueError('missing symptom run receipt')
        for index, (record, summary) in enumerate(zip(run_records, summaries), 1):
            encoded = json.dumps(record['records'], sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()
            if (record['run'] != index or record['sha256'] != hashlib.sha256(encoded).hexdigest()
                    or summary != {key: value for key, value in record.items() if key != 'records'}):
                raise ValueError('symptom run receipt changed')
    if (not isinstance(result, dict) or result.get("status") not in ("completed", "infrastructure_error", "not_available")
            or not isinstance(result.get("tests"), list) or not isinstance(result.get("note"), str)
            or not isinstance(result.get("totals"), dict) or set(result["totals"]) != set(OUTCOMES)
            or any(type(count) is not int or count < 0 for count in result["totals"].values())
            or any(not isinstance(item, dict) or not isinstance(item.get("test"), str)
                   or item.get("outcome") not in OUTCOMES for item in result["tests"])):
        raise ValueError("Invalid summary structure")
    # #823: raw non-symptom facts still cross the former action envelope
    # before receipt decoration. An oversized proof must retain its refusal.
    # Declared symptoms already crossed the unchanged legacy action bound.
    if result.get('declared_symptom') is None:
        result = bounded(result, limit=SUMMARY_BYTES - 2048)
    return result


def make_test_tool(worktree, settings: TestSettings | None = None, *, emit=None, forcing=None, timeout_seconds=None, declared_symptom=None, _checks=None, role='maker', check_index=0):
    """Bind settings from role inputs, directly or via worktree.test_settings."""
    settings = settings if settings is not None else getattr(worktree, "test_settings", None)
    emit = emit if emit is not None else getattr(worktree, "progress_emit", lambda event: None)
    regressions = tuple(test for check in getattr(worktree, "test_checks", ())
                        if check["kind"] == "regression" for test in check["tests"])

    def finish(result, base, base_sha, tree, selected, proof):
        if base:
            result["base_sha"] = base_sha
            result["base_semantics"] = "base_subject_head_observer" if proof and proof["mode"] == "head-observer" else "untouched_base"
            result["demonstrating_proof"] = "pending_checking"
            if result["status"] == "completed" and result.get("proof") != proof:
                raise ExecutionFailure(ExecutionOutcome.TRANSPORT_FAILURE, "summary", "proof_mismatch")
        if tree is not None:
            result["tree"] = tree
        if forcing is not None and result["status"] == "completed" and (
                result.get("forcing") != forcing.as_json()
                or not selection_matches(result.get("selectors"), selected)):
            raise ExecutionFailure(ExecutionOutcome.TRANSPORT_FAILURE, "summary", "forcing_or_selection_mismatch")
        if declared_symptom is not None and result.get("declared_symptom") != declared_symptom:
            raise ExecutionFailure(ExecutionOutcome.TRANSPORT_FAILURE, "summary", "symptom_mismatch")
        return bounded(result, limit=SUMMARY_BYTES)

    def run(tests, base=False, repeat=1, parallel=0, priority=None):
        selected = _selectors([] if tests is None else tests)
        repeat, parallel = options(repeat, parallel)
        priority = _priority(priority)
        key = None
        phase = "admission"
        try:
            if not isinstance(settings, TestSettings):
                return infrastructure("Test settings were not supplied by the role.")
            flux_execution.activation(settings)
            base_sha = getattr(worktree, "check_base_sha", None) if base else None
            if base and (not isinstance(base_sha, str)
                         or not re.fullmatch(r"(?:[0-9a-f]{40}|[0-9a-f]{64})", base_sha)
                         or getattr(worktree, "test_checkout", None) is None):
                return infrastructure("The role did not supply a check base and checkout.")
            call, root = flux_execution.allocate(settings)
            checks = [check for check in (getattr(worktree, 'test_checks', ()) if _checks is None else _checks)
                      if any(selects(selector, test) or selects(test, selector)
                             for selector in selected for test in check['tests'])]
            tree = _stage(worktree, root, base_sha, checks=checks, forcing=forcing,
                          declared_symptom=declared_symptom, selected=selected, role=role, check_index=check_index)
            proof = json.loads((root / 'proof.json').read_text()) if base else None
            contract = {"tree": tree, "trusted": _git(root, "write-tree", "--prefix=trusted/").strip(),
                        "image": settings.image, "cpus": settings.cpus, "mem_gb": settings.mem_gb,
                        "quiet_seconds": settings.quiet_seconds, "scratch": scratch_environment(),
                        "selectors": selected, "base_sha": base_sha, "repeat": repeat,
                        "parallel": parallel, "priority": priority, "timeout_seconds": timeout_seconds,
                        "forcing": forcing.as_json() if forcing is not None else None,
                        "declared_symptom": declared_symptom, "proof": proof}
            identity = hashlib.sha256(json.dumps(contract, sort_keys=True).encode()).hexdigest()
            worktree.pb_identity = identity
            # Stage first. An old checkpoint is usable only for this immutable
            # input, selection, resources and trusted executor implementation.
            reattachments = getattr(worktree, "pb_reattach", {})
            reattach = reattachments.get(base)
            if (isinstance(reattach, dict)
                    and (reattach.get("identity") != identity or reattach.get("tree") != tree)):
                key = reattach.get("key")
                raise ExecutionFailure(ExecutionOutcome.TRANSPORT_FAILURE, "reattachment", "checkpoint_identity_changed")
            resumed = isinstance(reattach, dict)
            if resumed:
                key = reattach['key']
                plan = flux_execution.resume(settings, key, tree, contract)
            else:
                arguments = [json.dumps(selected)]
                if forcing is not None:
                    arguments.extend(['--forcing', json.dumps(forcing.as_json(), sort_keys=True)])
                if declared_symptom is not None:
                    arguments.extend(['--declared-symptom', json.dumps(declared_symptom, sort_keys=True)])
                if repeat != 1:
                    arguments.extend(['--repeat', str(repeat)])
                if parallel:
                    arguments.extend(['--parallel', str(parallel)])
                plan = flux_execution.prepare(settings, call, root, tree, contract,
                    parallel=parallel, walltime=timeout_seconds or 86400, arguments=arguments)
                key = plan['key']
                reattachments[base] = {'backend': 'flux', 'key': key, 'tree': tree, 'identity': identity}
                worktree.pb_reattach = reattachments
                getattr(worktree, 'pb_submitted', lambda key, base, tree: None)(key, base, tree)
            phase = 'retrieval'
            payload = flux_execution.execute(plan, resumed=resumed, emit=emit)
            if payload['kind'] != 'tests':
                raise ValueError('Flux returned a different worker protocol')
            result = _verified_test_summary(payload['summary'], payload['symptom_runs'])
            result.update(wait_reference=key, receipts=payload['receipts'])
            if payload['symptom_runs']:
                result['symptom_receipts'] = payload['symptom_receipts']
            result = finish(bounded(model_result(result), limit=SUMMARY_BYTES - 512),
                            base, base_sha, tree, selected, proof)
            reattachments.pop(base, None)
            return result
        except ProofRefusal as error:
            return {'status': 'not_available', 'message': str(error), 'tests': [],
                    'totals': dict.fromkeys(OUTCOMES, 0), 'note': '',
                    'proof_refusal': {'rule': error.rule, 'module': error.module, 'symbol': error.symbol,
                                      'reason': error.reason, 'violations': error.as_json()}}
        except ExecutionFailure as failure:
            result = infrastructure(failure.message())
            result.update(execution=failure.as_json(), selectors=list(selected))
            if key:
                result["wait_reference"] = key
            return bounded(result, limit=SUMMARY_BYTES)
        except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
            failure = ExecutionFailure(ExecutionOutcome.TRANSPORT_FAILURE, phase, "unavailable_or_unverified")
            result = infrastructure(failure.message())
            result.update(execution=failure.as_json(), selectors=list(selected))
            if key:
                result["wait_reference"] = key
            return bounded(result, limit=SUMMARY_BYTES)

    def run_tests(tests: list[str] | None = None, base: bool = False, repeat: int = 1,
                  parallel: int = 0, priority: int | None = None) -> dict:
        """Run pytest with each planned base partition; Checking owns the proof verdict."""
        if base and _checks is None:
            selected = _selectors([] if tests is None else tests)
            checks = [check for check in getattr(worktree, 'test_checks', ())
                      if not selected or any(selects(selector, test) or selects(test, selector)
                             for selector in selected for test in check['tests'])]
            if checks:
                groups = []
                for check in checks:
                    original_index = next(index for index, declared in enumerate(worktree.test_checks) if declared is check)
                    nodes = [test for test in check['tests'] if not selected or any(selects(selector, test) or selects(test, selector) for selector in selected)]
                    symptom = check.get('declared_symptom')
                    tool = make_test_tool(worktree, settings, emit=emit, timeout_seconds=timeout_seconds,
                                          forcing=Forcing.parse(check['forcing'], check_index=original_index, role=role) if check.get('forcing') else None,
                                          declared_symptom=symptom, role=role, check_index=original_index, _checks=({**check, 'tests': nodes},))
                    observed = tool(nodes, base=True, repeat=symptom['repeat'] if symptom else repeat,
                                    parallel=symptom['parallel'] if symptom else parallel, priority=priority)
                    groups.append({'check': check.get('name', 'planned check'), **observed})
                # Broad selectors also ask for tests outside the plan. Run those on
                # untouched base, separately from the declared proof campaigns.
                remainder = [selector for selector in selected
                             if not any(selects(test, selector) for check in checks for test in check['tests'])]
                if remainder or not selected:
                    groups.append({'check': 'unplanned base tests', **make_test_tool(worktree, settings, emit=emit,
                        timeout_seconds=timeout_seconds, role=role, _checks=())(remainder, base=True, repeat=repeat, parallel=parallel, priority=priority)})
                if len(groups) == 1:
                    return {key: value for key, value in groups[0].items() if key != 'check'}
                status = next((item['status'] for item in groups if item['status'] != 'completed'), 'completed')
                return bounded({'status': status, 'groups': groups,
                                'tests': [test for item in groups for test in item['tests']],
                                'receipts': [receipt for item in groups for receipt in item.get('receipts', [])],
                                'totals': {outcome: sum(item['totals'][outcome] for item in groups) for outcome in OUTCOMES},
                                'note': 'Each group runs with its own declared source partition and execution settings.'}, limit=SUMMARY_BYTES)
        result = run(tests, base, repeat, parallel, priority)
        if (base or result["status"] != "completed"
                or result.get("execution", {}).get("outcome") == ExecutionOutcome.TEST_TIMEOUT):
            return result
        failed = [item for item in result["tests"] if item["outcome"] in ("failed", "error")]
        if not failed:
            return result
        # One plain base run says whether a failure is new; it keeps the request's priority.
        baseline = run_tests([item["test"] for item in failed], True, priority=priority)
        result["base"] = {key: value for key, value in baseline.items() if key != "tests"}
        outcomes = {item["test"]: item for item in baseline["tests"]}
        for item in failed:
            before = outcomes.get(item["test"])
            outcome = "unverified"
            if baseline["status"] == "completed":
                outcome = before["outcome"] if before else "missing"
                if before and before["message"].startswith("Selector not found:"):
                    outcome = "missing"
                if not before and baseline["note"]:
                    outcome = "unverified"
            item["base_outcome"] = outcome
            if outcome in ("failed", "error"):
                item["classification"] = (
                    "pre-existing" if any(selects(test, item["test"]) for test in regressions)
                    else "fails-on-base"
                )
            else:
                item["classification"] = "new" if outcome in ("passed", "missing") else "unverified"
        return bounded(result, limit=SUMMARY_BYTES)

    return run_tests
