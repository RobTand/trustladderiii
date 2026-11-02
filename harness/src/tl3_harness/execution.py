from dataclasses import asdict, dataclass, replace
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time
from typing import Protocol

from .limits import MAX_DETAILED_FAILURES, MAX_MESSAGE_CHARS, MAX_REPORT_BYTES, MAX_REPORT_LINE_BYTES, validate_check_plan
from .model import Check, HarnessInputError, Kind, Outcome, Result, TestResult, Verdict, judge
from .node_ids import selects
from .report import InvalidReport, decode, parse
from . import selection
from .trees import (UnsupportedEntry, added_names, changed_configuration, changed_existing_tests,
                    changed_files, changed_protected, commit, configuration, export, is_test, own_base,
                    test_edits)


TMPFS_MIB = 512
# The one check the harness adds to a plan: existing tests that import the change (#359).
SELECTED_CHECK = "Existing tests that import the change"
MAX_SELECTED_TESTS = 1000


_BASE_RESULTS: dict[tuple, dict[str, Outcome]] = {}


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
             messages: dict[str, str] | None = None) -> dict[str, Outcome]:
    missing = {test for test in tests if not (tree / test.split("::")[0]).is_file()}
    records = []
    invalid = False
    record_limit = 4 * len(tests) + 16
    line_limit = MAX_REPORT_LINE_BYTES
    retained_bytes = 0
    # The container gets no writable report mount or PB token. Its event stream
    # is journalled and parsed by this host process, outside the checkout.
    with report.open("wb") as journal, tempfile.TemporaryFile() as errors, tempfile.TemporaryFile() as selection:
        selection.write(json.dumps(tests).encode())
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
            messages: dict[str, str] | None = None) -> dict[str, Outcome]: ...


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

    def run(self, tree, container, tests, report, progress, additions=frozenset(), *, messages=None):
        return run_tree(tree, container, tests, report, progress, additions, messages=messages)


PYTEST = Pytest()


def selected_candidates(repo: Path, tool: TestTool, old: Path, new: Path, config: dict, base: str,
                        head: str, plan: tuple[Check, ...], limit: int) -> tuple[str, ...]:
    """Existing tests the change's imports reach, closest first, up to `limit`. Tests the plan
    names are judged there; tests the head edits or lacks are the leaf's own, for review."""
    named = [test for check in plan for test in check.tests]
    candidates = [test for test in tool.select(old, changed_files(repo, own_base(repo, base, head), head), config)
                  if not any(selects(test, other) or selects(other, test) for other in named)]
    edited = tool.changed_existing_tests(old, new, tuple(candidates))
    candidates = [test for test in candidates if test not in edited][:limit]
    while candidates:
        try:
            validate_check_plan(plan + (Check(SELECTED_CHECK, Kind.REGRESSION, tuple(candidates)),))
            break
        except ValueError:
            candidates = candidates[:len(candidates) // 2]
    return tuple(candidates)


def run_checks(repo: Path, base: str, head: str, plan: tuple[Check, ...], *,
               container: Container, reviewed: str | None = None, tool: TestTool = PYTEST,
               protected: tuple[str, ...] = (), selected_tests: int | None = None) -> tuple[Result, ...]:
    """Run one round inside one admitted PB action, from trusted installed code. `protected`
    holds the repository's protected path patterns; a change to any of them is refused.
    With `selected_tests`, up to that many existing tests whose imports reach the change,
    and that pass on the base, run as one more regression check, `SELECTED_CHECK`."""
    require_admission()
    if not plan or len({check.name for check in plan}) != len(plan):
        raise HarnessInputError("a plan needs uniquely named checks")
    if any(check.name == SELECTED_CHECK for check in plan):
        raise HarnessInputError("a planned check may not use the harness's selected-check name")
    if selected_tests is not None and (type(selected_tests) is not int
                                       or not 1 <= selected_tests <= MAX_SELECTED_TESTS):
        raise HarnessInputError(f"selected_tests must be 1 to {MAX_SELECTED_TESTS}")
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
        config = tool.configuration(old)
        if refusal := configuration_changes(repo, root, base, old, head, new, tool, protected):
            return refused(plan, refusal)
        invalid = {test for test in tests if not tool.is_test(test.split("::")[0], config)}
        if invalid:
            outcomes = dict.fromkeys(tests, Outcome.UNCOLLECTED)
            messages = dict.fromkeys(tests, "The plan names a test outside the configured test files.")
            return tuple(judge(check, outcomes, outcomes, messages=messages) for check in plan)
        ordinary = tuple(dict.fromkeys(test for check in plan
                                       if check.kind not in (Kind.DEMONSTRATING, Kind.REPRODUCING)
                                       for test in check.tests))
        changed = tool.changed_existing_tests(old, new, ordinary)
        demonstrating = tuple(dict.fromkeys(test for check in plan if check.kind == Kind.DEMONSTRATING
                                            for test in check.tests))
        candidates = (selected_candidates(repo, tool, old, new, config, base, head, plan, selected_tests)
                      if selected_tests else ())
        progress = Progress()
        base_tests = tuple(dict.fromkeys(ordinary + candidates))
        cache_key = (os.environ["PRISMABUILD_CONTAINER_OWNER"], base, container, base_tests)
        if base_tests and cache_key not in _BASE_RESULTS:
            observed = tool.run(old, container, base_tests, root / "base.jsonl", progress)
            if Outcome.NO_REPORT not in observed.values():
                _BASE_RESULTS[cache_key] = observed
        before = _BASE_RESULTS.get(cache_key, dict.fromkeys(base_tests, Outcome.NO_REPORT)).copy()
        # A test that doesn't pass on the base says nothing about the change: drop it, don't excuse it.
        kept = tuple(test for test in candidates if before.get(test) == Outcome.PASSED)
        if demonstrating:
            shutil.copytree(old, demo)
            tool.test_edits(old, new, demo, config)
            additions = tool.added_names(old, new, config)
            demonstrated = tool.run(demo, container, demonstrating, root / "demonstrating.jsonl", progress, additions)
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
                                  root / "reproducing.jsonl", progress, additions)
        messages = {}
        after = tool.run(new, container, tuple(dict.fromkeys(tests + kept)), root / "head.jsonl", progress,
                         messages=messages)
        comparisons = {Kind.DEMONSTRATING: demonstrated, Kind.REPRODUCING: reproduced,
                       Kind.REGRESSION: before, Kind.BEHAVIOR_PRESERVING: before}
        results = []
        for check in plan:
            result = judge(check, comparisons[check.kind], after, messages=messages)
            if check.kind in (Kind.REGRESSION, Kind.BEHAVIOR_PRESERVING):
                restored = tuple(replace(test, passed=False, message=(
                    f"Restore the existing test `{test.test}`; existing tests may not be changed to satisfy a check.")[:1000])
                    if test.test in changed else test for test in result.tests)
                if any(test.test in changed for test in result.tests):
                    result = replace(result, verdict=Verdict.FAIL, tests=restored)
            results.append(result)
        if kept:
            results.append(judge(Check(SELECTED_CHECK, Kind.REGRESSION, kept), before, after, messages=messages))
        return tuple(results)


def result_json(results: tuple[Result, ...]) -> str:
    reports = [asdict(result) for result in results]
    detailed = 0
    for report in reports:
        for test in report['tests']:
            if not test['passed'] and detailed < MAX_DETAILED_FAILURES:
                test['message'] = test['message'][:MAX_MESSAGE_CHARS].encode(errors='backslashreplace').decode()[:MAX_MESSAGE_CHARS]
                detailed += 1
            else:
                test['message'] = ''
    return json.dumps(reports, sort_keys=True, separators=(',', ':'))
