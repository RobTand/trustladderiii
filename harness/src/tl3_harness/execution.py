from dataclasses import asdict, dataclass, replace
import json
import hashlib
import os
from pathlib import Path
import re
import shutil
import tempfile
import time
from typing import Protocol

from .limits import MAX_DETAILED_FAILURES, MAX_MESSAGE_CHARS, MAX_REPORT_BYTES, MAX_REPORT_LINE_BYTES, MAX_RESULT_BYTES, RESULT_ENVELOPE_BYTES, validate_check_plan
from .model import (ALWAYS_CHECK, SELECTED_CHECK, Check, FailureNotes, Forcing, HarnessInputError, Kind, Outcome, Result, TestResult, Verdict,
                    NATIVE_THREAD_ENVIRONMENT, judge)
from .node_ids import validate_always_tests
from .report import InvalidReport, decode, parse
from .supervision import ExecutionFailure, ExecutionOutcome, Supervised
from . import selection
from .trees import (UnsupportedEntry, added_names, changed_configuration, changed_existing_tests,
                    changed_files, changed_protected, commit, configuration, export, is_test, own_base,
                    test_edits, helper_test_edits, validate_helper_setup)


TMPFS_MIB = 512
MAX_SELECTED_TESTS = 1000


_BASE_RESULTS: dict[tuple, tuple[dict[str, Outcome], dict[str, str]]] = {}
# Complete execution units, never individual members, apart from the plan's base cache.
_SELECTED_BASE: dict[tuple, "SelectedObservation"] = {}


@dataclass
class SelectedRun:
    groups: tuple[tuple[str, ...], ...]
    limit: int
    expected: tuple[str, ...] | None = None
    collected: tuple[str, ...] = ()
    refusal: str = ""
    prefix: bool = False
    allow_empty: bool = False
    saturation: dict | None = None

    def as_json(self):
        return {"groups": self.groups, "limit": self.limit, "expected": self.expected,
                "prefix": self.prefix, "allow_empty": self.allow_empty}


@dataclass(frozen=True)
class SelectedObservation:
    outcomes: dict[str, Outcome]
    collection: tuple[str, ...]

    @property
    def identity(self):
        return hashlib.sha256(json.dumps(self.collection, ensure_ascii=True).encode()).hexdigest()


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
        *[arg for name in NATIVE_THREAD_ENVIRONMENT
          for arg in ("--env", f"{name}={os.environ.get(name, '1')}")],
        "--entrypoint=python3",
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
             messages: dict[str, str] | None = None, forcing: Forcing | None = None,
             selected: SelectedRun | None = None) -> dict[str, Outcome]:
    missing = {test for test in tests if not (tree / test.split("::")[0]).is_file()}
    records = []
    invalid = False
    record_limit = 4 * len(tests) + 16
    line_limit = MAX_REPORT_LINE_BYTES
    retained_bytes = 0
    # The container gets no writable report mount or PB token. Its event stream
    # is journalled and parsed by this host process, outside the checkout.
    with report.open("wb") as journal, tempfile.TemporaryFile() as selection:
        # The forcing travels with the selection to the read-only runner, outside the checkout (#393).
        request = tests
        if forcing is not None:
            request = {"tests": tests, "forcing": forcing.as_json()}
        if selected is not None:
            request = {"tests": tests, "selection": selected.as_json()}
        selection.write(json.dumps(request).encode())
        selection.seek(0)
        try:
            with Supervised(container_command(tree, container, tests), selection, tests) as process:
                for line in process.records():
                    retained_bytes += len(line)
                    if len(line) > line_limit or len(records) > record_limit or retained_bytes > MAX_REPORT_BYTES:
                        invalid = True
                        break
                    journal.write(line)
                    journal.flush()
                    os.fsync(journal.fileno())
                    try:
                        record = decode(line)
                    except InvalidReport:
                        invalid = True
                        break
                    records.append(record)
                    if record.get("event") == "collection" and isinstance(record.get("tests"), list):
                        record_limit = 4 * len(record["tests"]) + 16
                        line_limit = min(MAX_REPORT_LINE_BYTES, max((len(json.dumps(test, ensure_ascii=False).encode(errors='backslashreplace'))
                                                                   for test in record['tests']), default=0) + 8192)
                    if record.get("event") == "phase" and record.get("when") == "teardown":
                        progress.commit(record.get("test", ""))
                exit_code = process.exit_code
        except ExecutionFailure as failure:
            if messages is not None:
                messages.update(dict.fromkeys(tests, failure.message()))
            # UNCOLLECTED cannot be assertion RED, GREEN, or an allowed pre-existing error.
            outcome = Outcome.UNCOLLECTED if failure.outcome == ExecutionOutcome.TEST_TIMEOUT else Outcome.NO_REPORT
            return dict.fromkeys(tests, outcome)
        except ValueError:
            return dict.fromkeys(tests, Outcome.NO_REPORT)
    try:
        if invalid:
            raise InvalidReport("invalid report stream")
        outcomes = parse(records, tests, missing, exit_code, additions, messages=messages)
        if selected is not None:
            selected.collected = tuple(test for record in records if record.get("event") == "collection"
                                       for test in record["tests"])
            selected.refusal = next((record["message"] for record in records
                                     if record.get("event") == "plan-error"), "")
            selected.saturation = next((record["saturation"] for record in records
                                        if record.get("event") == "collection" and "saturation" in record), None)
        return outcomes
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
            messages: dict[str, str] | None = None, forcing: Forcing | None = None,
            selected: SelectedRun | None = None) -> dict[str, Outcome]: ...


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
        return selection.select(tree, changed, lambda name: is_test(name, config["test-files"]), modules_only=True)

    def run(self, tree, container, tests, report, progress, additions=frozenset(), *, messages=None, forcing=None,
            selected=None):
        if selected is not None:
            return run_tree(tree, container, tests, report, progress, additions, messages=messages, selected=selected)
        # Test seams stand in for run_tree without a forcing parameter; pass it only when set.
        if forcing is None:
            return run_tree(tree, container, tests, report, progress, additions, messages=messages)
        return run_tree(tree, container, tests, report, progress, additions, messages=messages, forcing=forcing)


PYTEST = Pytest()


def selected_candidates(repo: Path, tool: TestTool, old: Path, new: Path, config: dict, base: str,
                        head: str, plan: tuple[Check, ...], limit: int, failures: tuple[str, ...] = ()) -> tuple[tuple[str, ...], ...]:
    """Closest complete modules, or owner-declared groups of modules. Planned functions
    stay in their owning unit. Historical failures precede import candidates, even
    outside the current import closure. An edited unit is excluded whole for review."""
    declared = {name: tuple(group) for group in config.get("selection-groups", []) for name in group}
    named_files = {test for check in plan for test in check.tests if "::" not in test}
    candidates, seen = [], set()
    try:
        for test in (*failures, *tool.select(old, changed_files(repo, own_base(repo, base, head), head), config)):
            name = test.split("::", 1)[0]
            group = declared.get(name, (name,))
            if name in seen:
                continue
            seen.update(group)
            known_failure = any(test.split("::", 1)[0] in group for test in failures)
            if (set(group) <= named_files and not known_failure) or tool.changed_existing_tests(old, new, group):
                continue
            candidates.append(group)
        if candidates:
            validate_check_plan(plan + (Check(SELECTED_CHECK, Kind.REGRESSION,
                                             tuple(member for group in candidates for member in group)),))
    except (RecursionError, MemoryError):
        return ()
    except ValueError as error:
        raise HarnessInputError(f"Selected execution groups exceed report limits: {error}") from error
    return tuple(candidates)


def selected_base(tool: TestTool, old: Path, container: Container, candidates: tuple[tuple[str, ...], ...], base: str,
                  root: Path, progress: Progress, limit: int, config: dict,
                  required: tuple[tuple[str, ...], ...] = ()) -> tuple[dict, dict | None]:
    """Admit a priority prefix at actual collected costs. Cached units consume budget
    only when their turn arrives; base failures do not refund their collection cost.
    Required historical units must fit before any ordinary candidate is admitted."""
    key = (os.environ["PRISMABUILD_CONTAINER_OWNER"], base, container, id(tool),
           json.dumps(config, sort_keys=True), "complete-prefix-v2", limit,
           tuple((name, os.environ.get(name, default)) for name, default in NATIVE_THREAD_ENVIRONMENT.items()))
    before, charged, index, saturation = {}, 0, 0, None
    while index < len(candidates):
        group = candidates[index]
        remaining = limit - charged
        cached = _SELECTED_BASE.get((*key, group))
        if (remaining == 0 and group not in required) or (cached is not None and len(cached.collection) > remaining):
            if group in required:
                cost = charged + len(cached.collection)
                raise HarnessInputError(f"Previously failing execution units need at least {cost} tests; "
                                        f"selected-test budget is {limit}. Cannot skip {group}.")
            saturation = {"unit": list(group), "cases": len(cached.collection) if cached else None,
                          "remaining": remaining, "reason": "Next complete unit cannot fit; lower-priority units were not checked."}
            break
        if cached is not None:
            before[group] = cached
            charged += len(cached.collection)
            index += 1
            continue
        # Batch uncached units using static lower bounds only to bound collection work.
        # The runner admits the actual complete prefix before executing any item.
        batch, lower_bound = [], 0
        for candidate in candidates[index:]:
            if batch and (candidate in required) != (group in required):
                break
            if (*key, candidate) in _SELECTED_BASE:
                break
            batch.append(candidate)
            try:
                lower_bound += sum(max(1, len(selection.node_ids(old, member))) for member in candidate)
            except selection.UNREADABLE as error:
                raise HarnessInputError(f"Cannot discover the complete selected execution group {candidate}.") from error
            if lower_bound >= remaining:
                break
        contract = SelectedRun(tuple(batch), remaining, prefix=True, allow_empty=bool(before))
        tests = tuple(member for unit in batch for member in unit)
        observed = tool.run(old, container, tests, root / f"selected-base-{index}.jsonl", progress, selected=contract)
        if contract.refusal:
            prefix = f"Previously failing execution units cannot fit the {limit}-test budget: " if group in required else ""
            raise HarnessInputError(prefix + contract.refusal)
        if contract.saturation is None and not contract.collected and any(value in (Outcome.NO_REPORT, Outcome.UNCOLLECTED)
                                          for value in observed.values()):
            saturation = {"unit": list(batch[0]), "cases": None, "remaining": remaining,
                          "reason": "Base collection unverified; unknown consumed budget cannot be reused. "
                                    "No checks of the remaining suffix were verified."}
            break
        for candidate in batch:
            if contract.saturation and list(candidate) == contract.saturation['unit']:
                if candidate in required:
                    cost = charged + contract.saturation['cases']
                    raise HarnessInputError(f"Previously failing execution units need at least {cost} tests; "
                                            f"selected-test budget is {limit}. Cannot skip {candidate}.")
                saturation = {**contract.saturation,
                              "reason": "Next complete unit cannot fit; lower-priority units were not checked."}
                break
            collection = tuple(test for test in contract.collected if test.split("::", 1)[0] in candidate)
            outcomes = {member: observed.get(member, Outcome.NO_REPORT) for member in candidate}
            observation = SelectedObservation(outcomes, collection)
            before[candidate] = observation
            if collection and Outcome.NO_REPORT not in outcomes.values():
                _SELECTED_BASE[(*key, candidate)] = observation
            charged += len(collection)
            index += 1
        if saturation:
            break
    receipt = None
    if saturation:
        receipt = {"policy": "historical failures first, then complete-unit prefix by import distance, path, and declaration order",
                   "limit": limit, "charged_cases": charged,
                   "base_units": [{"members": group, "cases": len(observation.collection),
                                   "outcomes": observation.outcomes} for group, observation in before.items()],
                   "head_units": [], "skipped_units": candidates[index:], "skipped_checked": False,
                   "saturation": saturation}
    return before, receipt


def selected_head(tool: TestTool, new: Path, container: Container, kept: tuple[tuple[str, ...], ...],
                  before: dict[tuple[str, ...], SelectedObservation], root: Path, progress: Progress,
                  limit: int) -> Result:
    """Run the same complete units as base. Retry each failing unit once, within the
    original node budget and the detailed-failure cap. Never isolate a group member."""
    tests = tuple(member for group in kept for member in group)
    expected = tuple(test for group in kept for test in before[group].collection)
    contract = SelectedRun(kept, limit, expected)
    messages, notes = {}, {}
    after = tool.run(new, container, tests, root / "selected-head.jsonl", progress,
                     messages=messages, selected=contract)
    if contract.refusal:
        raise HarnessInputError(contract.refusal)
    head_collections = {group: tuple(test for test in contract.collected if test.split("::", 1)[0] in group)
                        for group in kept}
    failing = [group for group in kept if any(after.get(member) != Outcome.PASSED for member in group)]
    retried = set()
    for index, group in enumerate(failing[:MAX_DETAILED_FAILURES]):
        retry_messages = {}
        retry_contract = SelectedRun((group,), len(before[group].collection), before[group].collection)
        retry = tool.run(new, container, group, root / f"selected-retry-{index}.jsonl", progress,
                         messages=retry_messages, selected=retry_contract)
        if retry_contract.refusal:
            raise HarnessInputError(retry_contract.refusal)
        head_collections[group] = retry_contract.collected
        retried.add(group)
        passed = all(retry.get(member) == Outcome.PASSED for member in group)
        for member in group:
            if passed:
                notes[member] = ("Failed on the head with the selected groups, then passed on retry as a "
                                 "complete execution group: flaky or disturbed by another group. "
                                 "Not counted against the change.")
            elif member in retry_messages:
                messages[member] = retry_messages[member]
            after[member] = retry.get(member, Outcome.NO_REPORT)
    base_outcomes = {member: before[group].outcomes[member] for group in kept for member in group}
    failed_members = set()
    for group in kept:
        if any(after.get(member) != Outcome.PASSED for member in group):
            failed_members.update(group)
            for member in group:
                if after.get(member) == Outcome.PASSED:
                    notes[member] = ("This module passed, but a companion in its complete execution group failed. "
                                     "Editing any companion excludes this unresolved failed group from selection.")
        identity = before[group].identity
        membership = ("identical base/head collection" if head_collections[group] == before[group].collection
                      else "head collection unverified; required base collection")
        receipt = (f"Complete execution group {group}; {membership} "
                   f"{len(before[group].collection)} tests, sha256 {identity}; "
                   f"retry unit: {group if group in retried else 'not retried'}.")
        for member in group:
            if after.get(member, Outcome.NO_REPORT) == Outcome.NO_REPORT:
                after[member] = Outcome.ERROR
                messages[member] = ("The execution group ended its process on the head without a complete "
                                    "report; this complete group passed on the base.")
            notes[member] = (receipt + "\n" + (notes.get(member) or messages.get(member, ""))).strip()[:MAX_MESSAGE_CHARS]
    result = judge(Check(SELECTED_CHECK, Kind.REGRESSION, tests), base_outcomes, after, messages=messages)
    return replace(result, tests=tuple(replace(test, passed=test.passed and test.test not in failed_members,
                                              message=notes[test.test]) for test in result.tests))


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
               always_tests: tuple[str, ...] = (), selected_failures: tuple[str, ...] = ()) -> tuple[Result, ...]:
    """Run one round inside one admitted PB action, from trusted installed code. `protected`
    holds the repository's protected path patterns; a change to any of them is refused.
    With `selected_tests`, complete execution groups whose imports reach the change
    run as one more regression check, `SELECTED_CHECK`. The limit counts collected cases.
    `always_tests` runs the configured files or node IDs as regression checks every round.
    `selected_failures` is controller-owned history; complete historical units run first."""
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
        if not isinstance(selected_failures, (tuple, list)):
            raise ValueError("selected_failures must list previous selected test identities")
        for failure in selected_failures:
            validate_always_tests((failure,))
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
        candidates = (selected_candidates(repo, tool, old, new, config, base, head, plan, selected_tests, selected_failures)
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
        selected_before, selection_receipt = selected_base(tool, old, container, candidates, base, root, progress, selected_tests, config,
                                                          tuple(group for group in candidates if any(
                                                              test.split("::", 1)[0] in group for test in selected_failures)))
        # Filter complete units: no head run may lose a fixture companion.
        kept = tuple(group for group in selected_before
                     if all(outcome == Outcome.PASSED for outcome in selected_before[group].outcomes.values()))
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
                                "to fail by assertion on both helpers; setup/runtime errors and successful returns do not count.")
                        for test in result.tests))
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
            results.append(selected_head(tool, new, container, kept, selected_before, root, progress, selected_tests))
        if selection_receipt is not None:
            selection_receipt["head_units"] = kept
            results[-1] = replace(results[-1], selection=selection_receipt)
        return tuple(results)


def helper_failure_preservation(tool, staged, head, evidence, container, root, progress):
    """Call the real target with bounded data or existing setup, independently of observers."""
    module, function = evidence.target.split("::")
    setups = validate_helper_setup((staged / module).read_text(), evidence)
    source = """import inspect
import json
import shutil
import pytest
from pathlib import Path
from types import SimpleNamespace
# Select pytest's implementations even if the served repository overrides them.
from _pytest.tmpdir import tmp_path_factory  # noqa: F401


@pytest.fixture
def _tl3_fixture_context(request, tmp_path_factory):
    original_mktemp = tmp_path_factory.mktemp
    original_basetemp = tmp_path_factory.getbasetemp
    root = original_mktemp('tl3-helper')
    allocated = 0
    def mktemp(*args, **kwargs):
        nonlocal allocated
        if allocated >= 16:
            raise RuntimeError('Helper setup exceeds 16 temporary directories per case')
        allocated += 1
        return original_mktemp(*args, **kwargs)
    tmp_path_factory.getbasetemp = lambda: root
    tmp_path_factory.mktemp = mktemp
    try:
        yield request
    finally:
        tmp_path_factory.mktemp = original_mktemp
        tmp_path_factory.getbasetemp = original_basetemp
        shutil.rmtree(root)


@pytest.fixture
def tmp_path(_tl3_fixture_context, tmp_path_factory):
    return tmp_path_factory.mktemp('path')


def argument(value, module, witnesses, request):
    if isinstance(value, dict) and '$setup' in value:
        name = value['$setup']
        try:
            factory = getattr(module, name)
            bound = inspect.signature(factory).bind_partial()
            bound.arguments.update({name: request.getfixturevalue(name) for name in SETUPS[name]['fixtures']})
            bound.apply_defaults()
            result = factory(*bound.args, **bound.kwargs)
            method = value['$call']
            original = getattr(result, method)
            if not callable(original):
                raise TypeError('declared method is not callable')
            witness = {'returned': False}
            def observed(*args, **kwargs):
                outcome = original(*args, **kwargs)
                witness['returned'] = True
                return outcome
            setattr(result, method, observed)
            witnesses.append((result, method, original, witness))
            return result
        except Exception as error:
            raise RuntimeError(f'Helper setup {name} failed: {type(error).__name__}') from error
    if isinstance(value, dict) and '$attributes' in value:
        return SimpleNamespace(**value['$attributes'])
    return value
"""
    source += "\nSETUPS = " + repr(setups) + "\n"
    for index, encoded in enumerate(evidence.failure_cases):
        source += f"""

def test_failure_{index}(_tl3_fixture_context):
    request = _tl3_fixture_context
    module = pytest.Module.from_parent(request.session, path=Path({module!r}).resolve()).obj
    target = getattr(module, {function!r})
    case = json.loads({encoded!r})
    witnesses = []
    try:
        args = [argument(value, module, witnesses, request) for value in case['args']]
        kwargs = {{key: argument(value, module, witnesses, request) for key, value in case['kwargs'].items()}}
        try:
            target(*args, **kwargs)
        except AssertionError:
            if any(not witness['returned'] for _, _, _, witness in witnesses):
                raise RuntimeError('Helper setup did not reach the declared method return')
            return
        raise AssertionError('The known-bad helper invocation no longer fails by assertion')
    finally:
        for value, method, original, _ in reversed(witnesses):
            setattr(value, method, original)
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
    # Owner rule notes retain their existing priority. Actual failures precede group
    # companion notes and passing receipts, without increasing the shared allowance.
    messages = [(result['name'], test) for result in reports for test in result['tests']]
    priority = sorted(messages, key=lambda pair: (
        pair[0] != ALWAYS_CHECK,
        pair[1]['passed'] or pair[1]['head'] == Outcome.PASSED,
        pair[1]['passed']))
    retained = {id(test) for _, test in [pair for pair in priority if pair[1]['message']][:MAX_DETAILED_FAILURES]}
    for result, report in zip(results, reports):
        # Unforced results keep their earlier shape; a forced one carries its forcing for review (#393).
        if result.selection is None:
            del report["selection"]
        if result.helper_evidence is None:
            del report["helper_evidence"]
        if result.forcing is None:
            del report['forcing']
        else:
            report['forcing'] = result.forcing.as_json()
        for test in report['tests']:
            if test['head_forced'] is None:
                del test['head_forced']
            if id(test) in retained:
                test['message'] = test['message'][:MAX_MESSAGE_CHARS].encode(errors='backslashreplace').decode()[:MAX_MESSAGE_CHARS]
            else:
                test['message'] = ''
    encoded = json.dumps(reports, sort_keys=True, separators=(',', ':'))
    if len(encoded.encode()) > MAX_RESULT_BYTES - RESULT_ENVELOPE_BYTES:
        raise HarnessInputError('Selection receipt exceeds the existing result byte budget.')
    return encoded
