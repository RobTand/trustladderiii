from dataclasses import asdict, dataclass
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time

from .model import Check, Kind, Outcome, Result, judge
from .report import InvalidReport, decode, parse
from .trees import added_names, commit, configuration, export, is_test, test_edits, validate_configuration


@dataclass(frozen=True)
class Container:
    image: str

    def __post_init__(self):
        if not re.fullmatch(r"(?:[\w./:-]+@)?sha256:[0-9a-f]{64}", self.image):
            raise ValueError("use an immutable image ID or manifest digest")


def container_command(tree: Path, container: Container, tests: tuple[str, ...]) -> list[str]:
    harness = Path(__file__).resolve().parent
    return [
        "docker", "run", "--rm", "--pull=never", "--network=none", "--read-only",
        "--cap-drop=ALL", "--security-opt=no-new-privileges", "--user=65534:65534",
        "--pids-limit=128", "--ipc=private", "--workdir=/checkout",
        "--mount", f"type=bind,src={tree},dst=/checkout,readonly",
        "--mount", f"type=bind,src={harness},dst=/harness,readonly",
        "--tmpfs", "/tmp:rw,nosuid,nodev,size=256m,mode=1777",
        "--env", "HOME=/tmp", "--env", "TMPDIR=/tmp", "--env", "PYTEST_DISABLE_PLUGIN_AUTOLOAD=1",
        "--env", "OMP_NUM_THREADS=1", "--env", "MKL_NUM_THREADS=1",
        "--env", "OPENBLAS_NUM_THREADS=1", "--entrypoint=python3",
        container.image, "-I", "-B", "/harness/runner.py", json.dumps(tests),
    ]


def require_admission():
    required = ("PRISMABUILD_CONTAINER_OWNER", "PRISMABUILD_CONTAINER_MARKER",
                "PRISMABUILD_ACTION_PROGRESS_PATH", "PRISMABUILD_ACTION_PROGRESS_TOKEN")
    if not all(os.environ.get(name) for name in required):
        raise RuntimeError("checks require an admitted PrismaBuild action with a progress contract")
    docker = shutil.which("docker")
    if not docker or "Docker CLI shim" not in Path(docker).read_text(errors="replace")[:200]:
        raise RuntimeError("PrismaBuild's managed Docker shim must be first on PATH")


class Progress:
    def __init__(self):
        self.units = 0

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
             progress: Progress, additions: frozenset[tuple[str, str]] = frozenset()) -> dict[str, Outcome]:
    missing = {test for test in tests if not (tree / test.split("::")[0]).is_file()}
    records = []
    invalid = False
    # The container gets no writable report mount or PB token. Its event stream
    # is journalled and parsed by this host process, outside the checkout.
    with report.open("wb") as journal, tempfile.TemporaryFile() as errors:
        with subprocess.Popen(container_command(tree, container, tests), stdout=subprocess.PIPE,
                              stderr=errors) as child:
            while line := child.stdout.readline(65537):
                if len(line) > 65536 or len(records) > 4 * len(tests) + 16:
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
                if record.get("event") == "phase" and record.get("when") == "teardown":
                    progress.commit(record.get("test", ""))
            exit_code = child.wait()
    try:
        if invalid:
            raise InvalidReport("invalid report stream")
        return parse(records, tests, missing, exit_code, additions)
    except InvalidReport:
        return dict.fromkeys(tests, Outcome.NO_REPORT)


def run_checks(repo: Path, base: str, head: str, plan: tuple[Check, ...], *,
               container: Container) -> tuple[Result, ...]:
    """Run one round inside one admitted PB action, from trusted installed code."""
    require_admission()
    if not plan or len({check.name for check in plan}) != len(plan):
        raise ValueError("a plan needs uniquely named checks")
    repo = repo.resolve()
    if Path(__file__).resolve().is_relative_to(repo):
        raise ValueError("the installed harness must be outside the served checkout")
    base, head = commit(repo, base), commit(repo, head)
    with tempfile.TemporaryDirectory(prefix="tl3-checks-") as directory:
        root = Path(directory)
        root.chmod(0o755)
        old, new, demo = [root / name for name in ("base", "head", "demonstrating")]
        export(repo, base, old)
        export(repo, head, new)
        config = configuration(old)
        validate_configuration(old, new)
        tests = tuple(dict.fromkeys(test for check in plan for test in check.tests))
        if any(not is_test(test.split("::")[0], config["test-files"]) for test in tests):
            raise ValueError("a named test is outside the configured test files")
        ordinary = tuple(dict.fromkeys(test for check in plan if check.kind != Kind.DEMONSTRATING
                                       for test in check.tests))
        demonstrating = tuple(dict.fromkeys(test for check in plan if check.kind == Kind.DEMONSTRATING
                                            for test in check.tests))
        progress = Progress()
        before = run_tree(old, container, ordinary, root / "base.jsonl", progress) if ordinary else {}
        if demonstrating:
            shutil.copytree(old, demo)
            test_edits(old, new, demo, config["test-files"])
            additions = added_names(old, new, config["test-files"])
            demonstrated = run_tree(demo, container, demonstrating, root / "demonstrating.jsonl", progress, additions)
        else:
            demonstrated = {}
        after = run_tree(new, container, tests, root / "head.jsonl", progress)
        return tuple(judge(check, demonstrated if check.kind == Kind.DEMONSTRATING else before, after)
                     for check in plan)


def result_json(results: tuple[Result, ...]) -> str:
    return json.dumps([asdict(result) for result in results], sort_keys=True)
