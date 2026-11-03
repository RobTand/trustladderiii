from dataclasses import asdict, dataclass, replace
import json
import hashlib
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time
from typing import Protocol

from .limits import MAX_DETAILED_FAILURES, MAX_MESSAGE_CHARS, MAX_REPORT_BYTES, MAX_REPORT_LINE_BYTES, validate_check_plan
from .model import (ALWAYS_CHECK, Check, FailureNotes, Forcing, HarnessInputError, Kind, Outcome, Result, TestResult, Verdict,
                    judge)
from .node_ids import selects, validate_always_tests
from .report import InvalidReport, decode, parse
from . import selection
from .trees import (UnsupportedEntry, added_names, changed_configuration, changed_existing_tests,
                    changed_files, changed_protected, commit, configuration, export, is_test, own_base,
                    test_edits, helper_test_edits)


TMPFS_MIB = 512
# Existing tests that import the change (#359), and configured repo-wide rules (#498).
SELECTED_CHECK = "Existing tests that import the change"
MAX_SELECTED_TESTS = 1000


_BASE_RESULTS: dict[tuple, tuple[dict[str, Outcome], dict[str, str]]] = {}
# Selected candidates' base outcomes, per test, apart from the plan's base cache.
_SELECTED_BASE: dict[tuple, Outcome] = {}


@dataclass(frozen=True)
class Container:
    image: str
    exec_scratch: bool = False

    def __post_init__(self):
        if type(self.exec_scratch) is not bool:
            raise HarnessInputError("exec_scratch must be a boolean")
        if not re.fullmatch(r"(?:[\w./:-]+@)?sha256:[0-9a-f]{64}", self.image):
            raise HarnessInputError("use an immutable image ID or manifest digest")


def container_command(tree: Path, container: Container, tests: tuple[str, ...]) -> list[str]:
    harness = Path(__file__).resolve().parent
    return [
        "docker", "run", "--interactive", "--rm", "--pull=never", "--network=none", "--read-only",
        "--cap-drop=ALL", "--security-opt=no-new-privileges", "--user=65534:65534",
        "--pids-limit=128", "--ipc=private", "--workdir=/checkout",
        "--mount", f"type=bind,src={tree},dst=/checkout,readonly",
        "--mount", f"type=bind,src={harness},dst=/harness,readonly",
        "--tmpfs", f"/tmp:rw,nosuid,nodev,{'exec' if container.exec_scratch else 'noexec'},size={TMPFS_MIB}m,mode=1777",
        "--env", "HOME=/tmp", "--env", "TMPDIR=/tmp", "--env", "PYTEST_DISABLE_PLUGIN_AUTOLOAD=1",
        "--env", "OMP_NUM_THREADS=1", "--env", "MKL_NUM_THREADS=1",
        "--env", "OPENBLAS_NUM_THREADS=1", "--entrypoint=python3",
        container.image, "-I", "-B", "/harness/runner.py", "-",
    ]


def require_admission():
    required = ("PRISMABUILD_CONTAINER_OWNER", "PRISMABUILD_CONTAINER_MARKER",
                "PRISMABUILD_ACTION_PROGRESS_PATH", "PRISMABUILD_ACTION_PROGRESS_TOKEN")
    if not all(os.environ.get(name) for name in required):
        raise RuntimeError("checks require an admitted PrismaBuild action with a progress contract")
    phases = json.loads(os.environ.get("PRISMABUILD_ACTION_PROGRESS_ALLOWANCES", "{}"))
    if "tests" not in phases:
        raise RuntimeError("declare the PrismaBuild progress phase 'tests'")
    docker = shutil.which("docker")
    if not docker or "Docker CLI shim" not in Path(docker).read_text(errors="replace")[:200]:
        raise RuntimeError("PrismaBuild's managed Docker shim must be first on PATH")


class Progress:
    def __init__(self):
        self.units = 0
        path = os.environ.get("PRISMABUILD_ACTION_PROGRESS_PATH")
        if path and Path(path).exists():
            record = json.loads(Path(path).read_text())
            if (record.get("token") != os.environ.get("PRISMABUILD_ACTION_PROGRESS_TOKEN")
                    or type(record.get("units_completed")) is not int or record["units_completed"] < 0):
                raise RuntimeError("invalid existing PrismaBuild progress record")
            self.units = record["units_completed"]

    def commit(self, test: str):
        self.units += 1
        path = Path(os.environ["PRISMABUILD_ACTION_PROGRESS_PATH"])
        record = {"schema": "prismabuild.action_progress.v1",
                  "token": os.environ["PRISMABUILD_ACTION_PROGRESS_TOKEN"],
                  "phase": "tests", "units_completed": self.units,
                  "unit": test, "reported_unix": time.time()}
        temporary = path.with_name(f"{path.name}.{os.getpid()}.tmp")
        with temporary.open("w") as target:
            json.dump(record, target, sort_keys=True)
            target.write("\n")
            target.flush()
            os.fsync(target.fileno())
        temporary.replace(path)


def run_tree(tree: Path, container: Container, tests: tuple[str, ...], report: Path,
             progress: Progress, additions: frozenset[tuple[str, str]] = frozenset(), *,
             messages: dict[str, str] | None = None, forcing: Forcing | None = None) -> dict[str, Outcome]:
    missing = {test for test in tests if not (tree / test.split("::")[0]).is_file()}
    records = []
    invalid = False
    record_limit = 4 * len(tests) + 16
    line_limit = MAX_REPORT_LINE_BYTES
    retained_bytes = 0
    # The container gets no writable report mount or PB token. Its event stream
    # is journalled and parsed by this host process, outside the checkout.
    with report.open("wb") as journal, tempfile.TemporaryFile() as errors, tempfile.TemporaryFile() as selection:
        # The forcing travels with the selection to the read-only runner, outside the checkout (#393).
        selection.write(json.dumps(tests if forcing is None else
                                   {"tests": tests, "forcing": forcing.as_json()}).encode())
        selection.seek(0)
        with subprocess.Popen(container_command(tree, container, tests), stdin=selection,
                              stdout=subprocess.PIPE, stderr=errors) as child:
            while line := child.stdout.readline(line_limit + 1):
                retained_bytes += len(line)
                if len(line) > line_limit or len(records) > record_limit or retained_bytes > MAX_REPORT_BYTES:
                    invalid = True
                    child.terminate()
                    break
                journal.write(line)
                journal.flush()
                os.fsync(journal.fileno())
                try:
                    record = decode(line)
                except InvalidReport:
                    invalid = True
                    child.terminate()
                    break
                records.append(record)
                if record.get("event") == "collection" and isinstance(record.get("tests"), list):
                    record_limit = 4 * len(record["tests"]) + 16
                    line_limit = min(MAX_REPORT_LINE_BYTES, max((len(json.dumps(test, ensure_ascii=False).encode(errors='backslashreplace'))
                                                               for test in record['tests']), default=0) + 8192)
                if record.get("event") == "phase" and record.get("when") == "teardown":
                    progress.commit(record.get("test", ""))
            exit_code = child.wait()
    try:
        if invalid:
            raise InvalidReport("invalid report stream")
        return parse(records, tests, missing, exit_code, additions, messages=messages)
    except InvalidReport:
        return dict.fromkeys(tests, Outcome.NO_REPORT)


def refused(plan: tuple[Check, ...], message: str) -> tuple[Result, ...]:
    """A failing round that tells the maker what to undo; nothing ran."""
    return tuple(Result(check.name, check.kind, Verdict.FAIL, tuple(
        TestResult(test, Outcome.UNCOLLECTED, Outcome.UNCOLLECTED, False, False, message[:MAX_MESSAGE_CHARS])
        for test in check.tests)) for check in plan)


def listed(instruction: str, changed: list[str]) -> str:
    """The instruction first, then as many files as fit; clipping never hides what to do."""
    for shown in range(len(changed), 0, -1):
        rest = len(changed) - shown
        message = instruction + ", ".join(changed[:shown]) + (f" and {rest} more." if rest else ".")
        if len(message) <= MAX_MESSAGE_CHARS:
            return message
    return f"{instruction}{len(changed)} files."


def changed_files_message(configuration: list[str], protected: list[str]) -> str:
    if set(protected) <= set(configuration):
        return listed("Restore the changed test configuration: checks never run a changed test "
                      "configuration. Changed: ", configuration)
    return listed("Restore the changed protected files: checks never run a change to this repository's "
                  "protected paths. Changed: ", sorted(set(configuration) | set(protected)))


def unsupported_entry_message(entry: UnsupportedEntry) -> str:
    return ("Replace this entry with a regular file or remove it: checks run only regular files and "
            f"directories, never symlinks or submodules. Entry: {entry.name}")


def configuration_changes(repo: Path, root: Path, base: str, base_tree: Path, head: str,
                          head_tree: Path, tool: "TestTool", protected: tuple[str, ...]) -> str | None:
    """Judge only what the head changed, against the commit it descends from. A head
    built on an older main than the check base must not answer for main's changes.
    Returns the refusal message, or None."""
    fork = own_base(repo, base, head)
    if fork != base:
        base_tree = root / f"fork-{fork}"
        if not base_tree.exists():
            export(repo, fork, base_tree)
    configuration = tool.changed_configuration(base_tree, head_tree)
    paths = changed_protected(repo, fork, head, protected)
    return changed_files_message(configuration, paths) if configuration or paths else None


class TestTool(Protocol):
    """How a served repository configures, transplants and runs its tests (#390). The
    round's trees, refusals and verdicts in run_checks are the same for every tool."""

    def configuration(self, tree: Path) -> dict: ...

    def changed_configuration(self, base: Path, head: Path) -> list[str]: ...

    def is_test(self, name: str, config: dict) -> bool: ...

    def test_edits(self, base: Path, head: Path, destination: Path, config: dict) -> None: ...

    def added_names(self, base: Path, head: Path, config: dict) -> frozenset[tuple[str, str]]: ...

    def changed_existing_tests(self, base: Path, head: Path, tests: tuple[str, ...]) -> set[str]: ...

    def select(self, tree: Path, changed: list[str], config: dict) -> list[str]: ...

    def run(self, tree: Path, container: Container, tests: tuple[str, ...], report: Path,
            progress: Progress, additions: frozenset[tuple[str, str]] = frozenset(), *,
            messages: dict[str, str] | None = None, forcing: Forcing | None = None) -> dict[str, Outcome]: ...


class Pytest:
    """The default tool: `[tool.tl3.harness]` in pyproject.toml, one container per tree."""

    def configuration(self, tree):
        return configuration(tree)

    def changed_configuration(self, base, head):
        return changed_configuration(base, head)

    def is_test(self, name, config):
        return is_test(name, config["test-files"])

    def test_edits(self, base, head, destination, config):
        test_edits(base, head, destination, config["test-files"])

    def added_names(self, base, head, config):
        return added_names(base, head, config["test-files"])

    def changed_existing_tests(self, base, head, tests):
        return changed_existing_tests(base, head, tests)

    def select(self, tree, changed, config):
        return selection.select(tree, changed, lambda name: is_test(name, config["test-files"]))

    def run(self, tree, container, tests, report, progress, additions=frozenset(), *, messages=None, forcing=None):
        # Test seams stand in for run_tree without a forcing parameter; pass it only when set.
        if forcing is None:
            return run_tree(tree, container, tests, report, progress, additions, messages=messages)
        return run_tree(tree, container, tests, report, progress, additions, messages=messages, forcing=forcing)


PYTEST = Pytest()


def selected_candidates(repo: Path, tool: TestTool, old: Path, new: Path, config: dict, base: str,
                        head: str, plan: tuple[Check, ...], limit: int) -> tuple[str, ...]:
    """Existing tests the change's imports reach, closest first, up to `limit`. Tests the plan
    names are judged there; tests the head edits or lacks are the leaf's own, for review."""
    named = [test for check in plan for test in check.tests]
    candidates = []
    try:
        # Lazily, closest first: a widely imported file can reach thousands of tests.
        for test in tool.select(old, changed_files(repo, own_base(repo, base, head), head), config):
            if (any(selects(test, other) or selects(other, test) for other in named)
                    or tool.changed_existing_tests(old, new, (test,))):
                continue
            candidates.append(test)
            if len(candidates) == limit:
                break
    except (RecursionError, MemoryError):
        return ()
    while candidates:
        try:
            validate_check_plan(plan + (Check(SELECTED_CHECK, Kind.REGRESSION, tuple(candidates)),))
            break
        except ValueError:
            candidates = candidates[:len(candidates) // 2]
    return tuple(candidates)


def selected_base(tool: TestTool, old: Path, container: Container, candidates: tuple[str, ...], base: str,
                  root: Path, progress: Progress) -> dict[str, Outcome]:
    """The candidates' base outcomes, from one run apart from the plan's, cached per test."""
    key = (os.environ["PRISMABUILD_CONTAINER_OWNER"], base, container)
    missing = tuple(test for test in candidates if (*key, test) not in _SELECTED_BASE)
    if missing:
        for test, outcome in tool.run(old, container, missing, root / "selected-base.jsonl", progress).items():
            if outcome != Outcome.NO_REPORT:
                _SELECTED_BASE[(*key, test)] = outcome
    return {test: _SELECTED_BASE.get((*key, test), Outcome.NO_REPORT) for test in candidates}


def selected_head(tool: TestTool, new: Path, container: Container, kept: tuple[str, ...],
                  before: dict[str, Outcome], root: Path, progress: Progress) -> Result:
    """Judge the kept tests on the head, apart from the plan's run. A failing test reruns once,
    alone: the maker can't fix a flake, and one test ending the process must not take the
    others with it. The first MAX_DETAILED_FAILURES failures rerun; the rest stand."""
    messages, notes = {}, {}
    after = tool.run(new, container, kept, root / "selected-head.jsonl", progress, messages=messages)
    failing = [test for test in kept if after.get(test) != Outcome.PASSED]
    for index, test in enumerate(failing[:MAX_DETAILED_FAILURES]):
        alone_messages = {}
        alone = tool.run(new, container, (test,), root / f"selected-alone-{index}.jsonl", progress,
                         messages=alone_messages).get(test, Outcome.NO_REPORT)
        if alone == Outcome.PASSED:
            notes[test] = (f"{after.get(test, Outcome.NO_REPORT)} on the head with the other selected tests, "
                           "then passed alone: flaky or disturbed by another test. Not counted against the change.")
        elif test in alone_messages:
            messages[test] = alone_messages[test]
        after[test] = alone
    for test in kept:
        if after.get(test, Outcome.NO_REPORT) == Outcome.NO_REPORT:
            # It passed on the base; ending the process on the head is the change's failure.
            after[test] = Outcome.ERROR
            messages[test] = "The test ended its process on the head without a report; it passed on the base."
    result = judge(Check(SELECTED_CHECK, Kind.REGRESSION, kept), before, after, messages=messages)
    return replace(result, tests=tuple(replace(test, message=notes[test.test][:MAX_MESSAGE_CHARS])
                                       if test.passed and test.test in notes else test for test in result.tests))


def stale_result(result: Result, stale: set[str]) -> Result:
    if result.name != ALWAYS_CHECK:
        return result
    tests = tuple(replace(test, passed=True, message=(
        f"Ignored stale always_tests entry `{test.test}`: missing on both the base and the head. "
        "The repository owner should update or remove it.")[:MAX_MESSAGE_CHARS])
        if test.test in stale else test for test in result.tests)
    verdict = (Verdict.INFRASTRUCTURE_FAILURE if result.verdict == Verdict.INFRASTRUCTURE_FAILURE else
               Verdict.PASS if all(test.passed for test in tests) else Verdict.FAIL)
    return replace(result, tests=tests, verdict=verdict)


def run_checks(repo: Path, base: str, head: str, plan: tuple[Check, ...], *,
               container: Container, reviewed: str | None = None, tool: TestTool = PYTEST,
               protected: tuple[str, ...] = (), selected_tests: int | None = None,
               always_tests: tuple[str, ...] = ()) -> tuple[Result, ...]:
    """Run one round inside one admitted PB action, from trusted installed code. `protected`
    holds the repository's protected path patterns; a change to any of them is refused.
    With `selected_tests`, up to that many existing tests whose imports reach the change,
    and that pass on the base, run as one more regression check, `SELECTED_CHECK`.
    `always_tests` runs the configured files or node IDs as regression checks every round."""
    require_admission()
    if not plan or len({check.name for check in plan}) != len(plan):
        raise HarnessInputError("a plan needs uniquely named checks")
    if any(check.name == SELECTED_CHECK for check in plan):
        raise HarnessInputError("a planned check may not use the harness's selected-check name")
    if selected_tests is not None and (type(selected_tests) is not int
                                       or not 1 <= selected_tests <= MAX_SELECTED_TESTS):
        raise HarnessInputError(f"selected_tests must be 1 to {MAX_SELECTED_TESTS}")
    if any(check.name == ALWAYS_CHECK for check in plan):
        raise HarnessInputError("a planned check may not use the harness's always-check name")
    try:
        validate_always_tests(always_tests)
    except ValueError as error:
        raise HarnessInputError(str(error)) from error
    if always_tests:
        plan += (Check(ALWAYS_CHECK, Kind.REGRESSION, tuple(dict.fromkeys(always_tests))),)
    tests = tuple(dict.fromkeys(test for check in plan for test in check.tests))
    try:
        validate_check_plan(plan)
    except ValueError as error:
        # limits.py loads standalone inside the container, so it raises plain ValueError.
        raise HarnessInputError(str(error)) from error
    reproducing = tuple(dict.fromkeys(test for check in plan if check.kind == Kind.REPRODUCING
                                      for test in check.tests))
    if reproducing and reviewed is None:
        raise HarnessInputError("a reproducing check needs the exact reviewed commit")
    repo = repo.resolve()
    if Path(__file__).resolve().is_relative_to(repo):
        raise HarnessInputError("the installed harness must be outside the served checkout")
    base, head = commit(repo, base), commit(repo, head)
    if reviewed is not None:
        reviewed = commit(repo, reviewed)
    with tempfile.TemporaryDirectory(prefix="tl3-checks-") as directory:
        root = Path(directory)
        root.chmod(0o755)
        old, new, demo = [root / name for name in ("base", "head", "demonstrating")]
        # The base is the repository's: its errors stay upstream. The head is the maker's.
        export(repo, base, old)
        try:
            export(repo, head, new)
        except UnsupportedEntry as entry:
            return refused(plan, unsupported_entry_message(entry))
        stale = {test for test in always_tests
                 if not (old / test.split("::")[0]).is_file() and not (new / test.split("::")[0]).is_file()}
        tests = tuple(test for test in tests if test not in stale)
        config = tool.configuration(old)
        if refusal := configuration_changes(repo, root, base, old, head, new, tool, protected):
            return tuple(stale_result(result, stale) for result in refused(plan, refusal))
        invalid = {test for test in tests if not tool.is_test(test.split("::")[0], config)}
        if invalid:
            outcomes = dict.fromkeys(tests, Outcome.UNCOLLECTED)
            outcomes.update(dict.fromkeys(stale, Outcome.MISSING))
            messages = dict.fromkeys(tests, "The plan names a test outside the configured test files.")
            return tuple(stale_result(judge(replace(check, forcing=None), outcomes, outcomes, messages=messages), stale)
                         for check in plan)
        ordinary = tuple(dict.fromkeys(test for check in plan
                                       if check.kind not in (Kind.DEMONSTRATING, Kind.REPRODUCING)
                                       for test in check.tests if test not in stale))
        changed = tool.changed_existing_tests(old, new, ordinary)
        demonstrating = tuple(dict.fromkeys(test for check in plan
                                            if check.kind == Kind.DEMONSTRATING and check.forcing is None and check.helper_evidence is None
                                            for test in check.tests))
        candidates = (selected_candidates(repo, tool, old, new, config, base, head, plan, selected_tests)
                      if selected_tests else ())
        helpers = [check for check in plan if check.helper_evidence is not None]
        if len(helpers) > 1:
            return refused(plan, "Helper evidence supports one target check per plan, with all its observers.")
        helper_runs = {}
        for check in helpers:
            staged = root / "helper-observers"
            shutil.copytree(old, staged)
            try:
                if tool is not PYTEST:
                    raise ValueError("helper evidence requires the pytest tool")
                if not check.helper_evidence.failure_cases:
                    raise ValueError("helper evidence requires known-bad failure-preservation cases")
                identity = helper_test_edits(old, new, staged, config["test-files"],
                                             check.helper_evidence, check.tests)
            except ValueError as error:
                return refused(plan, str(error))
            helper_runs[check.name] = staged, identity
        progress = Progress()
        base_notes = {Kind.DEMONSTRATING: FailureNotes(), Kind.REPRODUCING: FailureNotes()}
        cache_key = (os.environ["PRISMABUILD_CONTAINER_OWNER"], base, container, ordinary)
        if ordinary and cache_key not in _BASE_RESULTS:
            before_messages = {}
            observed = tool.run(old, container, ordinary, root / "base.jsonl", progress, messages=before_messages)
            if Outcome.NO_REPORT not in observed.values():
                _BASE_RESULTS[cache_key] = observed, before_messages
        before, before_messages = _BASE_RESULTS.get(cache_key, (dict.fromkeys(ordinary, Outcome.NO_REPORT), {}))
        before = before.copy()
        before.update(dict.fromkeys(stale, Outcome.MISSING))
        # Candidates run apart, so one crashing there can't cost the plan its base evidence.
        selected_before = selected_base(tool, old, container, candidates, base, root, progress)
        # A test that doesn't pass on the base says nothing about the change: drop it, don't excuse it.
        kept = tuple(test for test in candidates if selected_before[test] == Outcome.PASSED)
        if demonstrating:
            shutil.copytree(old, demo)
            tool.test_edits(old, new, demo, config)
            additions = tool.added_names(old, new, config)
            demonstrated = tool.run(demo, container, demonstrating, root / "demonstrating.jsonl", progress, additions,
                                     messages=base_notes[Kind.DEMONSTRATING])
        else:
            demonstrated = {}
        reproduced = {}
        if reproducing:
            reviewed_tree, round_tree = root / "reviewed", root / "reproducing"
            try:
                export(repo, reviewed, reviewed_tree)
            except UnsupportedEntry as entry:
                return refused(plan, unsupported_entry_message(entry))
            # `changed` holds the edited existing tests; keep it for the guard below.
            if refusal := configuration_changes(repo, root, base, old, reviewed, reviewed_tree, tool, protected):
                return refused(plan, refusal)
            shutil.copytree(reviewed_tree, round_tree)
            tool.test_edits(reviewed_tree, new, round_tree, config)
            additions = tool.added_names(reviewed_tree, new, config)
            reproduced = tool.run(round_tree, container, reproducing,
                                  root / "reproducing.jsonl", progress, additions,
                                  messages=base_notes[Kind.REPRODUCING])
        # A forced check runs the old test on the original base and the new test on the
        # head, both under its forcing; the ordinary head run is the unforced one (#393).
        forced = {}
        for index, check in enumerate(check for check in plan if check.forcing is not None):
            notes, head_messages = FailureNotes(), {}
            forced_base = tool.run(old, container, check.tests, root / f"forced-base-{index}.jsonl", progress,
                                   messages=notes, forcing=check.forcing)
            forced_head = tool.run(new, container, check.tests, root / f"forced-head-{index}.jsonl", progress,
                                   messages=head_messages, forcing=check.forcing)
            forced[check.name] = forced_base, notes, forced_head, head_messages
        helper_outcomes = {}
        for check in helpers:
            staged, identity = helper_runs[check.name]
            existing_messages = {}
            existing = tool.run(old, container, check.tests, root / "helper-original-base.jsonl", progress,
                                messages=existing_messages)
            if any(existing[test] not in (Outcome.MISSING, Outcome.UNCOLLECTED) or existing_messages.get(test)
                   for test in check.tests):
                results = refused(plan, "Helper observers must be absent from base collection; "
                                  "existing or unavailable tests cannot be replaced by observers.")
                if Outcome.NO_REPORT in existing.values():
                    results = tuple(replace(result, verdict=Verdict.INFRASTRUCTURE_FAILURE) for result in results)
                return results
            notes = FailureNotes()
            outcomes = tool.run(staged, container, check.tests, root / "helper-base.jsonl", progress, messages=notes)
            try:
                preservation = helper_failure_preservation(tool, staged, new, check.helper_evidence,
                                                          container, root, progress)
            except ValueError as error:
                return refused(plan, f"Invalid helper failure preservation: {error}")
            identity.update(preservation)
            helper_outcomes[check.name] = outcomes, notes, identity
        messages = {}
        after = tool.run(new, container, tests, root / "head.jsonl", progress, messages=messages)
        after.update(dict.fromkeys(stale, Outcome.MISSING))
        # A collection/plan refusal is also UNCOLLECTED, but proves nothing about missing nodes.
        stale.update(test for test in always_tests if "::" in test
                     and before[test] in (Outcome.MISSING, Outcome.UNCOLLECTED)
                     and after[test] in (Outcome.MISSING, Outcome.UNCOLLECTED)
                     and not before_messages.get(test) and not messages.get(test))
        comparisons = {Kind.DEMONSTRATING: demonstrated, Kind.REPRODUCING: reproduced,
                       Kind.REGRESSION: before, Kind.BEHAVIOR_PRESERVING: before}
        results = []
        for check in plan:
            if check.name in helper_outcomes:
                outcomes, notes, identity = helper_outcomes[check.name]
                result = judge(check, outcomes, after, messages=messages, base_notes=notes)
                # This mode requires an actual assertion, never a missing-name substitute.
                if any(outcomes[test] != Outcome.ASSERTION for test in check.tests):
                    result = replace(result, verdict=(Verdict.INFRASTRUCTURE_FAILURE if result.verdict == Verdict.INFRASTRUCTURE_FAILURE else Verdict.FAIL), tests=tuple(
                        replace(test, passed=False, message="Helper observers must fail by assertion on the old helper.")
                        if outcomes[test.test] != Outcome.ASSERTION else test for test in result.tests))
                if any(item[side] != Outcome.PASSED for item in identity["failure_preservation"]
                       for side in ("base", "head")):
                    infrastructure = any(item[side] == Outcome.NO_REPORT for item in identity["failure_preservation"]
                                         for side in ("base", "head"))
                    result = replace(result, verdict=Verdict.INFRASTRUCTURE_FAILURE if infrastructure else Verdict.FAIL, tests=tuple(
                        replace(test, passed=False, message="Failure preservation requires every known-bad invocation "
                                "to fail by assertion on both helpers.") for test in result.tests))
                results.append(replace(result, helper_evidence=identity))
                continue
            if check.name in forced:
                forced_base, notes, forced_head, head_messages = forced[check.name]
                results.append(judge(check, forced_base, after, messages=messages, base_notes=notes,
                                     forced=forced_head, forced_messages=head_messages))
                continue
            result = judge(check, comparisons[check.kind], after, messages=messages,
                           base_notes=base_notes.get(check.kind))
            if check.kind in (Kind.REGRESSION, Kind.BEHAVIOR_PRESERVING):
                restored = tuple(replace(test, passed=False, message=(
                    f"Restore the existing test `{test.test}`; existing tests may not be changed to satisfy a check.")[:1000])
                    if test.test in changed else test for test in result.tests)
                if any(test.test in changed for test in result.tests):
                    result = replace(result, verdict=Verdict.FAIL, tests=restored)
            results.append(stale_result(result, stale))
        if kept:
            results.append(selected_head(tool, new, container, kept, selected_before, root, progress))
        return tuple(results)


def helper_failure_preservation(tool, staged, head, evidence, container, root, progress):
    """Trusted driver calls the real target with data, independently of observers."""
    module, function = evidence.target.split("::")
    source = """import json
import runpy
import sys
from pathlib import Path
from types import SimpleNamespace


def argument(value):
    if isinstance(value, dict) and '$attributes' in value:
        return SimpleNamespace(**value['$attributes'])
    return value
"""
    for index, encoded in enumerate(evidence.failure_cases):
        source += f"""

def test_failure_{index}():
    sys.path.insert(0, str(Path({module!r}).resolve().parent))
    target = runpy.run_path({module!r})[{function!r}]
    case = json.loads({encoded!r})
    args = [argument(value) for value in case['args']]
    kwargs = {{key: argument(value) for key, value in case['kwargs'].items()}}
    try:
        target(*args, **kwargs)
    except AssertionError:
        return
    raise AssertionError('The known-bad helper invocation no longer fails by assertion')
"""
    name = '_tl3_helper_failure_preservation.py'
    tests = tuple(f'{name}::test_failure_{index}' for index in range(len(evidence.failure_cases)))
    outcomes = []
    for label, tree in (("base", staged), ("head", head)):
        path = tree / name
        if path.exists():
            raise ValueError("reserved helper failure-preservation driver path exists")
        path.write_text(source)
        try:
            observed = tool.run(tree, container, tests, root / f"helper-failure-{label}.jsonl", progress)
            outcomes.append([observed[test] for test in tests])
        finally:
            path.unlink()
    return {"failure_cases": [json.loads(case) for case in evidence.failure_cases],
            "failure_driver_sha256": hashlib.sha256(source.encode()).hexdigest(),
            "failure_preservation": [{"base": before, "head": after}
                                     for before, after in zip(*outcomes)]}


def result_json(results: tuple[Result, ...]) -> str:
    reports = [asdict(result) for result in results]
    detailed = 0
    # Owner-configured rule notes must survive even when planned failures fill the message budget.
    for result, report in sorted(zip(results, reports), key=lambda pair: pair[0].name != ALWAYS_CHECK):
        # Unforced results keep their earlier shape; a forced one carries its forcing for review (#393).
        if result.helper_evidence is None:
            del report["helper_evidence"]
        if result.forcing is None:
            del report['forcing']
        else:
            report['forcing'] = result.forcing.as_json()
        for test in report['tests']:
            if test['head_forced'] is None:
                del test['head_forced']
            # Passing messages include stale rule notes and selected reruns that found a flake.
            if (not test['passed'] or test['message']) and detailed < MAX_DETAILED_FAILURES:
                test['message'] = test['message'][:MAX_MESSAGE_CHARS].encode(errors='backslashreplace').decode()[:MAX_MESSAGE_CHARS]
                detailed += 1
            else:
                test['message'] = ''
    return json.dumps(reports, sort_keys=True, separators=(',', ':'))
