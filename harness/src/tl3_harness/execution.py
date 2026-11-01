from dataclasses import asdict, dataclass, replace
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time

from .limits import MAX_DETAILED_FAILURES, MAX_MESSAGE_CHARS, MAX_REPORT_BYTES, MAX_REPORT_LINE_BYTES, validate_check_plan
from .model import Check, Kind, Outcome, Result, Verdict, judge
from .report import InvalidReport, decode, parse
from .trees import added_names, changed_existing_tests, commit, configuration, export, is_test, test_edits, validate_configuration


_BASE_RESULTS: dict[tuple, dict[str, Outcome]] = {}


@dataclass(frozen=True)
class Container:
    image: str

    def __post_init__(self):
        if not re.fullmatch(r"(?:[\w./:-]+@)?sha256:[0-9a-f]{64}", self.image):
            raise ValueError("use an immutable image ID or manifest digest")


def container_command(tree: Path, container: Container, tests: tuple[str, ...]) -> list[str]:
    harness = Path(__file__).resolve().parent
    return [
        "docker", "run", "--interactive", "--rm", "--pull=never", "--network=none", "--read-only",
        "--cap-drop=ALL", "--security-opt=no-new-privileges", "--user=65534:65534",
        "--pids-limit=128", "--ipc=private", "--workdir=/checkout",
        "--mount", f"type=bind,src={tree},dst=/checkout,readonly",
        "--mount", f"type=bind,src={harness},dst=/harness,readonly",
        "--tmpfs", "/tmp:rw,nosuid,nodev,size=256m,mode=1777",
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


def run_checks(repo: Path, base: str, head: str, plan: tuple[Check, ...], *,
               container: Container, reviewed: str | None = None) -> tuple[Result, ...]:
    """Run one round inside one admitted PB action, from trusted installed code."""
    require_admission()
    if not plan or len({check.name for check in plan}) != len(plan):
        raise ValueError("a plan needs uniquely named checks")
    tests = tuple(dict.fromkeys(test for check in plan for test in check.tests))
    validate_check_plan(plan)
    reproducing = tuple(dict.fromkeys(test for check in plan if check.kind == Kind.REPRODUCING
                                      for test in check.tests))
    if reproducing and reviewed is None:
        raise ValueError("a reproducing check needs the exact reviewed commit")
    repo = repo.resolve()
    if Path(__file__).resolve().is_relative_to(repo):
        raise ValueError("the installed harness must be outside the served checkout")
    base, head = commit(repo, base), commit(repo, head)
    if reviewed is not None:
        reviewed = commit(repo, reviewed)
    with tempfile.TemporaryDirectory(prefix="tl3-checks-") as directory:
        root = Path(directory)
        root.chmod(0o755)
        old, new, demo = [root / name for name in ("base", "head", "demonstrating")]
        export(repo, base, old)
        export(repo, head, new)
        config = configuration(old)
        validate_configuration(old, new)
        invalid = {test for test in tests if not is_test(test.split("::")[0], config["test-files"])}
        if invalid:
            outcomes = dict.fromkeys(tests, Outcome.UNCOLLECTED)
            messages = dict.fromkeys(tests, "The plan names a test outside the configured test files.")
            return tuple(judge(check, outcomes, outcomes, messages=messages) for check in plan)
        ordinary = tuple(dict.fromkeys(test for check in plan
                                       if check.kind not in (Kind.DEMONSTRATING, Kind.REPRODUCING)
                                       for test in check.tests))
        changed = changed_existing_tests(old, new, ordinary)
        demonstrating = tuple(dict.fromkeys(test for check in plan if check.kind == Kind.DEMONSTRATING
                                            for test in check.tests))
        progress = Progress()
        cache_key = (os.environ["PRISMABUILD_CONTAINER_OWNER"], base, container.image, ordinary)
        if ordinary and cache_key not in _BASE_RESULTS:
            observed = run_tree(old, container, ordinary, root / "base.jsonl", progress)
            if Outcome.NO_REPORT not in observed.values():
                _BASE_RESULTS[cache_key] = observed
        before = _BASE_RESULTS.get(cache_key, dict.fromkeys(ordinary, Outcome.NO_REPORT)).copy()
        if demonstrating:
            shutil.copytree(old, demo)
            test_edits(old, new, demo, config["test-files"])
            additions = added_names(old, new, config["test-files"])
            demonstrated = run_tree(demo, container, demonstrating, root / "demonstrating.jsonl", progress, additions)
        else:
            demonstrated = {}
        reproduced = {}
        if reproducing:
            reviewed_tree, round_tree = root / "reviewed", root / "reproducing"
            export(repo, reviewed, reviewed_tree)
            validate_configuration(old, reviewed_tree)
            shutil.copytree(reviewed_tree, round_tree)
            test_edits(reviewed_tree, new, round_tree, config["test-files"])
            additions = added_names(reviewed_tree, new, config["test-files"])
            reproduced = run_tree(round_tree, container, reproducing,
                                  root / "reproducing.jsonl", progress, additions)
        messages = {}
        after = run_tree(new, container, tests, root / "head.jsonl", progress, messages=messages)
        comparisons = {Kind.DEMONSTRATING: demonstrated, Kind.REPRODUCING: reproduced,
                       Kind.REGRESSION: before, Kind.BEHAVIOR_PRESERVING: before}
        results = []
        for check in plan:
            result = judge(check, comparisons[check.kind], after, messages=messages)
            if check.kind in (Kind.REGRESSION, Kind.BEHAVIOR_PRESERVING):
                protected = tuple(replace(test, passed=False, message=(
                    f"Restore the existing test `{test.test}`; existing tests may not be changed to satisfy a check.")[:1000])
                    if test.test in changed else test for test in result.tests)
                if any(test.test in changed for test in result.tests):
                    result = replace(result, verdict=Verdict.FAIL, tests=protected)
            results.append(result)
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
