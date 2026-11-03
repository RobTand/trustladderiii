"""The run_tests factory. The role supplies the base and execution settings."""
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import secrets
import shutil
import subprocess
import sys
import tempfile

from tl3_harness import execution
from tl3_harness.model import NATIVE_THREAD_ENVIRONMENT
from tl3_harness.node_ids import selects
from tl3_harness.supervision import ExecutionFailure, ExecutionOutcome
from tl3_harness.test_tool import PREFIX, SUMMARY_BYTES, OUTCOMES, bounded, infrastructure, options
from tl3_harness.trees import export


@dataclass(frozen=True)
class TestSettings:
    image: str
    tools_path: Path
    cpus: int = 1
    mem_gb: int = 2
    quiet_seconds: int = 120
    exec_scratch: bool = False

    def __post_init__(self):
        if type(self.exec_scratch) is not bool:
            raise ValueError("exec_scratch must be a boolean")
        if not re.fullmatch(r"(?:[\w./:-]+@)?sha256:[0-9a-f]{64}", self.image):
            raise ValueError("Use an immutable container image digest")
        if not Path(self.tools_path).is_absolute():
            raise ValueError("PrismaBuild tools_path must be absolute")
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
    # PrismaBuild runs a higher priority sooner. TL3's checks run at -10; requests stay at or below default.
    if priority is not None and (type(priority) is not int or not -10 <= priority <= 0):
        raise ValueError("Use priority -10 to 0, or leave it out for PrismaBuild's default")
    return priority


def _stage(worktree, root, base_sha=None, *, action="test_tool"):
    harness = Path(execution.__file__).resolve().parent
    owner = os.fstat(worktree._root)
    if any(os.path.samestat(parent.stat(), owner) for parent in (harness, *harness.parents)):
        raise ValueError("Install the trusted harness outside the role worktree")
    checkout = root / "checkout"
    if base_sha is not None:
        export(worktree.test_checkout, base_sha, checkout)
    else:
        checkout.mkdir()
        with worktree._directory(()) as directory:
            for name, is_dir in worktree._walk(directory):
                target = checkout / name
                if is_dir:
                    target.mkdir()
                else:
                    with worktree._file(name) as source, target.open("wb") as destination:
                        shutil.copyfileobj(source.buffer, destination)
                        mode = os.fstat(source.fileno()).st_mode
                        os.fchmod(destination.fileno(), 0o755 if mode & 0o100 else 0o644)
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
    # PB snapshots a Git checkout. These fixed commands create its private input,
    # never invoking hooks or commands from the role's Git configuration.
    commands = [("init", "--quiet", "--template=")]
    maker_checkout = getattr(worktree, "test_checkout", None)
    if maker_checkout is not None:
        # PB's snapshot parent proves this action belongs to the maker's checkout.
        # Keep the private harness input dirty so PB seals it above that parent.
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


def _json_command(command):
    with tempfile.TemporaryFile() as output:
        result = subprocess.run(command, stdout=output, stderr=subprocess.DEVNULL)
        output.seek(0)
        raw = output.read(2_097_153)
    if len(raw) > 2_097_152:
        raise ValueError("PrismaBuild response exceeded its limit")
    value = json.loads(raw)
    if not isinstance(value, dict):
        raise ValueError("PrismaBuild response must be an object")
    return result.returncode, value


def _summary(ending, key):
    cause = "invalid_ending"
    try:
        if (ending["action_key"] != key or ending["status"] not in ("executed", "cache_hit")
                or ending["exit_code"] != 0 or not ending["succeeded"]):
            raise ValueError("PrismaBuild action did not succeed")
        cause = "unverified_receipt"
        evidence = ending["result"]
        if (evidence["verified"] is not True or not isinstance(evidence["receipt_sha256"], str)
                or not re.fullmatch("[0-9a-f]{64}", evidence["receipt_sha256"])):
            raise ValueError("PrismaBuild result lacks verified receipt evidence")
        cause = "invalid_log_receipt"
        log = evidence["stdout"]
        if type(log["bytes"]) is not int or not 0 <= log["bytes"] <= 1_048_576:
            raise ValueError("PrismaBuild result log exceeded its limit")
        with open(log["path"], "rb") as stream:
            raw = stream.read(1_048_577)
        if len(raw) != log["bytes"] or hashlib.sha256(raw).hexdigest() != log["sha256"]:
            raise ValueError("PrismaBuild result log changed")
        cause = "missing_unique_summary"
        records = [line[len(PREFIX):] for line in raw.decode().splitlines() if line.startswith(PREFIX)]
        if len(records) != 1:
            raise ValueError("PrismaBuild returned no unique test summary")
        cause = "invalid_summary"
        result = json.loads(records[0])
        if result.get('declared_symptom') is not None:
            run_records = [json.loads(line.removeprefix('TL3_SYMPTOM_RUN ')) for line in raw.decode().splitlines()
                           if line.startswith('TL3_SYMPTOM_RUN ')]
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
        result["action_key"] = key
        # What a maker cites and a reviewer can check against PrismaBuild's own record.
        result["receipts"] = [{"action_key": key, "status": ending["status"], "exit_code": ending["exit_code"],
                               "receipt_sha256": evidence["receipt_sha256"], "log_sha256": log["sha256"]}]
        receipt = result["receipts"][0]
        if type(ending.get("attempt")) is int and ending["attempt"] > 0:
            receipt["attempt"] = ending["attempt"]
        generation = (ending.get("publication") or {}).get("generation")
        if isinstance(generation, str) and re.fullmatch("[0-9a-f]{64}", generation):
            receipt["generation"] = generation
        return bounded(result, limit=SUMMARY_BYTES - 512)
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise ExecutionFailure(ExecutionOutcome.TRANSPORT_FAILURE, "summary", cause) from error


def _failure_receipt(ending, settings):
    """Keep failure diagnosis without copying PB's private scheduler metadata."""
    diagnostics = ending.get("diagnostics") or {}
    receipt = {"action_key": ending["action_key"], "status": ending["status"],
               "exit_code": ending.get("exit_code")}
    if receipt["exit_code"] is None:
        receipt["exit_code"] = diagnostics.get("returncode")
    reason = diagnostics.get("termination_reason") or ending.get("reason")
    # Older pbwait consumer endings omit termination_reason. Read only this
    # failed action's terminal record, from the same installed fleet layout
    # used by the controller, and require its publication to match the ending.
    publication = ending.get("publication") or {}
    published = publication.get("published_unix")
    if reason != "memory_limit_oom" and published is not None:
        path = Path(settings.tools_path).parent.parent / "pb-queue" / "failed" / f"{ending['action_key']}.json"
        try:
            with path.open("rb") as stream:
                raw = stream.read(2_097_153)
            record = json.loads(raw) if len(raw) <= 2_097_152 else {}
            if (isinstance(record, dict) and record.get("action_key") == ending["action_key"]
                    and record.get("published_unix") == published):
                reason = (record.get("detail") or {}).get("termination_reason") or reason
        except (OSError, ValueError, TypeError):
            pass
    if isinstance(reason, str) and re.fullmatch(r"[a-z][a-z0-9_]{0,127}", reason):
        receipt["termination_reason"] = reason
    if type(ending.get("attempt")) is int and ending["attempt"] > 0:
        receipt["attempt"] = ending["attempt"]
    generation = publication.get("generation")
    if isinstance(generation, str) and re.fullmatch("[0-9a-f]{64}", generation):
        receipt["generation"] = generation
    scope = (diagnostics.get("resource_profile") or {}).get("scope") or {}
    peak = scope.get("memory_peak_bytes")
    if type(peak) is int and peak >= 0:
        receipt["memory_peak_bytes"] = peak
    return receipt


def make_test_tool(worktree, settings: TestSettings | None = None, *, emit=None, forcing=None, timeout_seconds=None, declared_symptom=None):
    """Bind settings from role inputs, directly or via worktree.test_settings."""
    settings = settings if settings is not None else getattr(worktree, "test_settings", None)
    emit = emit if emit is not None else getattr(worktree, "progress_emit", lambda event: None)
    regressions = tuple(test for check in getattr(worktree, "test_checks", ())
                        if check["kind"] == "regression" for test in check["tests"])

    def wait(key, base, base_sha, tree, selected):
        # pbwait's observation allowance is not an execution deadline.
        # Reattach to the same action whenever that allowance expires.
        while True:
            emit({"event": "progress", "unit": "external_wait",
                  "wait": {"upstream": "prismabuild", "key": key}})
            code, waited = _json_command([
                sys.executable, str(Path(settings.tools_path) / "pbwait.py"),
                "--endings", "--until", "cleaned", key,
            ])
            if waited.get("schema") != "prismabuild.endings.v1":
                raise ValueError("PrismaBuild returned an invalid ending")
            if type(waited.get("complete")) is not bool:
                raise ValueError("PrismaBuild returned no completion flag")
            endings = waited["endings"]
            if len(endings) != 1 or endings[0]["action_key"] != key:
                raise ValueError("PrismaBuild returned another action")
            if waited["complete"]:
                if code or not endings[0].get("succeeded"):
                    receipt = _failure_receipt(endings[0], settings)
                    # A PB phase named 'tests', elapsed time, and missing reports prove
                    # no test start. Only the trusted supervised summary can prove that.
                    failure = ExecutionFailure(ExecutionOutcome.UNKNOWN_INCOMPLETE, "unknown",
                                               receipt.get("termination_reason", "incomplete_execution"))
                    result = infrastructure(failure.message())
                    result.update(action_key=key, receipts=[receipt], execution=failure.as_json(),
                                  selectors=list(selected))
                else:
                    result = _summary(endings[0], key)
                if base:
                    result["base_sha"] = base_sha
                    result["base_semantics"] = "untouched_base"
                    result["demonstrating_proof"] = "pending_checking"
                if tree is not None:
                    result["tree"] = tree
                if forcing is not None and result["status"] == "completed" and (
                        result.get("forcing") != forcing.as_json() or result.get("selectors") != list(selected)):
                    raise ExecutionFailure(ExecutionOutcome.TRANSPORT_FAILURE, "summary", "forcing_or_selection_mismatch")
                if declared_symptom is not None and result.get("declared_symptom") != declared_symptom:
                    raise ExecutionFailure(ExecutionOutcome.TRANSPORT_FAILURE, "summary", "symptom_mismatch")
                return bounded(result, limit=SUMMARY_BYTES)
            if endings[0]["status"] not in ("waiting", "executed", "cache_hit"):
                raise ValueError("PrismaBuild could not verify completion")

    def run(tests, base=False, repeat=1, parallel=0, priority=None):
        selected = _selectors([] if tests is None else tests)
        repeat, parallel = options(repeat, parallel)
        priority = _priority(priority)
        key = None
        phase = "admission"
        try:
            if not isinstance(settings, TestSettings):
                return infrastructure("PrismaBuild test settings were not supplied by the role.")
            base_sha = getattr(worktree, "check_base_sha", None) if base else None
            if base and (not isinstance(base_sha, str)
                         or not re.fullmatch(r"(?:[0-9a-f]{40}|[0-9a-f]{64})", base_sha)
                         or getattr(worktree, "test_checkout", None) is None):
                return infrastructure("The role did not supply a check base and checkout.")
            with tempfile.TemporaryDirectory(prefix="tl3-test-input-") as temporary:
                root = Path(temporary)
                tree = _stage(worktree, root, base_sha)
                contract = {"tree": tree, "trusted": _git(root, "write-tree", "--prefix=trusted/").strip(),
                            "image": settings.image, "cpus": settings.cpus, "mem_gb": settings.mem_gb,
                            "quiet_seconds": settings.quiet_seconds, "exec_scratch": settings.exec_scratch,
                            "selectors": selected, "base_sha": base_sha, "repeat": repeat,
                            "parallel": parallel, "priority": priority, "timeout_seconds": timeout_seconds,
                            "forcing": forcing.as_json() if forcing is not None else None,
                            "declared_symptom": declared_symptom}
                identity = hashlib.sha256(json.dumps(contract, sort_keys=True).encode()).hexdigest()
                worktree.pb_identity = identity
                # Stage first. An old checkpoint is usable only for this immutable
                # input, selection, resources and trusted executor implementation.
                reattach = getattr(worktree, "pb_reattach", {}).pop(base, None)
                if (isinstance(reattach, dict) and getattr(worktree, "pb_strict_reattach", False)
                        and (reattach.get("identity") != identity or reattach.get("tree") != tree)):
                    key = reattach.get("key")
                    raise ExecutionFailure(ExecutionOutcome.TRANSPORT_FAILURE, "reattachment", "checkpoint_identity_changed")
                if isinstance(reattach, dict) and reattach.get("identity") == identity and reattach.get("tree") == tree:
                    key = reattach["key"]
                    if not isinstance(key, str) or not re.fullmatch("[0-9a-f]{64}", key):
                        raise ValueError("Invalid checkpoint action key")
                    phase = "retrieval"
                    return wait(key, base, base_sha, tree, selected)
                # Each xdist worker reserves a core and the image's memory.
                command = [sys.executable, str(Path(settings.tools_path) / "pbrun.py"),
                           "--cwd", str(root), "--anywhere", "--cpus", str(max(settings.cpus, parallel)),
                           "--demand", f"mem_gb={settings.mem_gb * max(1, parallel)}",
                           *(("--priority", str(priority)) if priority is not None else ()),
                           "--container-image", settings.image,
                           "--progress-phase", f"tests={settings.quiet_seconds}",
                           *[arg for name in NATIVE_THREAD_ENVIRONMENT
                             for arg in ("--env", f"{name}={max(1, settings.cpus // max(1, parallel))}")],
                           "--detach", "--",
                           "python3", "-I", "-B", "action.py", settings.image, json.dumps(selected)]
                if forcing is not None:
                    command.extend(["--forcing", json.dumps(forcing.as_json(), sort_keys=True)])
                if timeout_seconds is not None:
                    command[2:2] = ["--timeout-s", str(timeout_seconds)]
                if settings.exec_scratch:
                    command.append("--exec-scratch")
                # Defaults add nothing, so a default call keeps its action identity.
                if declared_symptom is not None:
                    command.extend(["--declared-symptom", json.dumps(declared_symptom, sort_keys=True)])
                if repeat != 1:
                    command.extend(["--repeat", str(repeat)])
                if parallel:
                    command.extend(["--parallel", str(parallel)])
                # PrismaBuild replays a stored result for a known action key; repeats must run.
                if repeat > 1:
                    command.extend(["--fresh", secrets.token_hex(16)])
                code, submitted = _json_command(command)
                if code or submitted.get("schema") not in (
                        "prismabuild.submission.v1", "prismaquant.prismabuild.pbrun_detach.v1"):
                    raise ValueError("PrismaBuild refused submission")
                key = submitted["action_key"]
                if not isinstance(key, str) or not re.fullmatch("[0-9a-f]{64}", key):
                    key = None
                    raise ValueError("PrismaBuild returned an invalid action key")
                getattr(worktree, "pb_submitted", lambda key, base, tree: None)(key, base, tree)
                phase = "retrieval"
                result = wait(key, base, base_sha, tree, selected)
                return result
        except ExecutionFailure as failure:
            result = infrastructure(failure.message())
            result.update(execution=failure.as_json(), selectors=list(selected))
            if key:
                result["action_key"] = key
            return bounded(result, limit=SUMMARY_BYTES)
        except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
            failure = ExecutionFailure(ExecutionOutcome.TRANSPORT_FAILURE, phase, "unavailable_or_unverified")
            result = infrastructure(failure.message())
            result.update(execution=failure.as_json(), selectors=list(selected))
            if key:
                result["action_key"] = key
            return bounded(result, limit=SUMMARY_BYTES)

    def run_tests(tests: list[str] | None = None, base: bool = False, repeat: int = 1,
                  parallel: int = 0, priority: int | None = None) -> dict:
        """Run pytest; compare failures with untouched base, never demonstrating proof."""
        result = run(tests, base, repeat, parallel, priority)
        if (base or result["status"] != "completed"
                or result.get("execution", {}).get("outcome") == ExecutionOutcome.TEST_TIMEOUT):
            return result
        failed = [item for item in result["tests"] if item["outcome"] in ("failed", "error")]
        if not failed:
            return result
        # One plain base run says whether a failure is new; it keeps the request's priority.
        baseline = run([item["test"] for item in failed], True, priority=priority)
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
        while len(json.dumps(result).encode()) > SUMMARY_BYTES and result["tests"]:
            passing = next((i for i in reversed(range(len(result["tests"])))
                            if result["tests"][i]["outcome"] == "passed"), -1)
            result["tests"].pop(passing)
            result["note"] = "Truncated: test entries were cut. Totals include all head results."
        return result

    return run_tests
