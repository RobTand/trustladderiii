from dataclasses import asdict
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

import pytest

from tl3_harness.execution import Container, Progress, container_command, run_checks
from tl3_harness.model import Check, Proof, FailureNotes, Custody, Kind, Outcome, Verdict, judge
from tl3_harness.report import InvalidReport, decode, parse
from tl3_harness.proof import assemble_proof
from tl3_harness.trees import added_names, export, git, changed_configuration


FIXTURES = Path(__file__).parent / "fixtures"
RUNNER = Path(__file__).parents[1] / "src/tl3_harness/runner.py"
IMAGE = "sha256:" + "1" * 64


@pytest.fixture
def trees(tmp_path):
    base, head, demo = [tmp_path / name for name in ("base", "head", "demo")]
    shutil.copytree(FIXTURES / "round/base", base)
    shutil.copytree(base, head)
    shutil.copytree(FIXTURES / "round/head", head, dirs_exist_ok=True)
    names = tuple('tests/test_product.py::' + node.name for node in __import__('ast').parse(
        (head / 'tests/test_product.py').read_text()).body if isinstance(node, __import__('ast').FunctionDef)
        and node.name != 'ready')
    assemble_proof(base, head, demo, Check('observers', Kind.DEMONSTRATING, names), {'test-files': ['tests/*.py']})
    return base, head, demo


def run(tree, names, additions=frozenset(), *, notes=None):
    tests = tuple("tests/test_product.py::" + name for name in names)
    env = {"PATH": os.environ["PATH"], "PYTEST_DISABLE_PLUGIN_AUTOLOAD": "1"}
    request = {'tests': tests, 'red_message': notes.red_message} if notes is not None else tests
    child = subprocess.run([sys.executable, "-I", "-B", str(RUNNER), json.dumps(request), str(tree)],
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
    records = [decode(line) for line in child.stdout.splitlines()]
    missing = {test for test in tests if not (tree / test.split("::")[0]).exists()}
    try:
        return tests, parse(records, tests, missing, child.returncode, additions, messages=notes)
    except InvalidReport:
        return tests, dict.fromkeys(tests, Outcome.NO_REPORT)


@pytest.mark.parametrize("kind,name,expected", [
    (Kind.DEMONSTRATING, "test_demonstrates", Verdict.PASS),
    (Kind.DEMONSTRATING, "test_does_not_demonstrate", Verdict.FAIL),
    (Kind.DEMONSTRATING, "test_setup", Verdict.FAIL),
    (Kind.DEMONSTRATING, "test_runtime_error", Verdict.FAIL),
    (Kind.DEMONSTRATING, "test_new_name", Verdict.PASS),
    (Kind.REGRESSION, "test_regression_passes", Verdict.PASS),
    (Kind.REGRESSION, "test_regression_breaks", Verdict.FAIL),
    (Kind.REGRESSION, "test_pre_existing", Verdict.PASS),
    (Kind.BEHAVIOR_PRESERVING, "test_preserved", Verdict.PASS),
    (Kind.BEHAVIOR_PRESERVING, "test_regression_breaks", Verdict.FAIL),
    (Kind.REGRESSION, "test_skipped", Verdict.FAIL),
    (Kind.REGRESSION, "test_xfailed", Verdict.FAIL),
    (Kind.REGRESSION, "uncollected", Verdict.FAIL),
    (Kind.REGRESSION, "test_absent_name", Verdict.FAIL),
])
def test_real_pytest_verdicts(trees, kind, name, expected):
    base, head, demo = trees
    additions = added_names(base, head, ["tests/*.py"])
    notes = FailureNotes('old product value')
    tests, before = run(demo if kind == Kind.DEMONSTRATING else base, [name], additions, notes=notes)
    _, after = run(head, [name])
    result = judge(Check(name, kind, tests, proof=Proof((), (), "old product value") if kind in (Kind.DEMONSTRATING, Kind.REPRODUCING) else None), before, after, base_notes=notes)
    assert result.verdict == expected, asdict(result)
    assert result.custody == Custody.IN_PROCESS
    assert result.tests[0].pre_existing == (name == "test_pre_existing")


def test_only_named_observers_reach_demonstrating_base(trees):
    base, head, demo = trees
    assert (demo / "product.py").read_bytes() == (base / "product.py").read_bytes()
    assert (demo / "tests/test_product.py").read_bytes() == (head / "tests/test_product.py").read_bytes()
    _, ordinary = run(base, ["test_demonstrates"])
    _, demonstrating = run(demo, ["test_demonstrates"])
    assert list(ordinary.values()) == [Outcome.PASSED]
    assert list(demonstrating.values()) == [Outcome.ASSERTION]


@pytest.mark.parametrize("fixture,outcome", [("no_report.py", Outcome.NO_REPORT),
                                            ("collection_error.py", Outcome.ERROR)])
def test_broken_jobs(trees, fixture, outcome):
    base, _, _ = trees
    shutil.copyfile(FIXTURES / fixture, base / "tests/test_product.py")
    tests, results = run(base, ["test_demonstrates"])
    assert results[tests[0]] == outcome
    if outcome == Outcome.NO_REPORT:
        assert judge(Check("no report", Kind.REGRESSION, tests), results, results).verdict == Verdict.INFRASTRUCTURE_FAILURE


def test_missing_file(trees):
    base, _, _ = trees
    (base / "tests/test_product.py").unlink()
    tests, results = run(base, ["test_demonstrates"])
    assert results[tests[0]] == Outcome.MISSING


def test_setup_error_is_not_an_assertion(trees):
    base, _, _ = trees
    tests, results = run(base, ["test_setup"])
    assert results[tests[0]] == Outcome.ERROR


def test_progress_is_atomic_cumulative_and_contains_no_test_secrets(tmp_path, monkeypatch):
    target = tmp_path / "progress.json"
    monkeypatch.setenv("PRISMABUILD_ACTION_PROGRESS_PATH", str(target))
    monkeypatch.setenv("PRISMABUILD_ACTION_PROGRESS_TOKEN", "sentinel")
    progress = Progress()
    progress.commit("first")
    assert json.loads(target.read_text())["units_completed"] == 1
    progress.commit("second")
    record = json.loads(target.read_text())
    assert record["schema"] == "prismabuild.action_progress.v1"
    assert record["units_completed"] == 2
    assert record["phase"] == "tests"
    resumed = Progress()
    resumed.commit("third")
    assert json.loads(target.read_text())["units_completed"] == 3


@pytest.mark.parametrize("base", [Outcome.MISSING, Outcome.SKIPPED, Outcome.UNCOLLECTED])
def test_regression_cannot_excuse_an_unavailable_base_test(base):
    check = Check("unavailable", Kind.REGRESSION, ("test.py::test_x",))
    assert judge(check, {check.tests[0]: base}, {check.tests[0]: Outcome.PASSED}).verdict == Verdict.FAIL


def test_container_has_only_explicit_mounts_and_no_host_environment(tmp_path, monkeypatch):
    monkeypatch.setattr("tl3_harness.execution.scratch_directory", lambda: tmp_path / "scratch")
    (tmp_path / "scratch").mkdir()
    command = container_command(tmp_path, Container(IMAGE), ("tests/test_x.py::test_x",))
    assert command[:2] == ["docker", "run"]
    for flag in ("--network=none", "--read-only", "--cap-drop=ALL", "--pull=never"):
        assert flag in command
    mounts = [command[index + 1] for index, value in enumerate(command) if value == "--mount"]
    assert len(mounts) == 3 and sum(mount.endswith(",readonly") for mount in mounts) == 2
    assert not any("/mnt/shared" in mount or "docker.sock" in mount for mount in mounts)
    assert "--cpuset-cpus" not in command


def test_requires_pb_admission(tmp_path, monkeypatch):
    monkeypatch.delenv("PRISMABUILD_CONTAINER_OWNER", raising=False)
    with pytest.raises(RuntimeError, match="admitted"):
        run_checks(tmp_path, "base", "head", (), container=Container(IMAGE))


@pytest.mark.parametrize("test", ["-k", "../test.py::test_x", "/test.py::test_x", "test.py::"])
def test_rejects_selection_arguments(test):
    with pytest.raises(ValueError):
        Check("invalid", Kind.REGRESSION, (test,))


def test_rejects_mutable_image():
    with pytest.raises(ValueError):
        Container("python:3.14")


def test_report_rejects_duplicate_and_truncated_records(trees):
    with pytest.raises(InvalidReport):
        decode(b'{"event":"finish","event":"start"}')
    with pytest.raises(InvalidReport):
        parse([{"event": "start", "schema": 1}], ("test.py::test_x",), set(), 0)


@pytest.mark.parametrize("middle", [
    [{"event": "collection", "tests": [{}]}],
    [{"event": "collection", "tests": ["other.py::test_x"]}],
    [{"event": "collection", "tests": ["test.py::test_x", "test.py::test_x"]}],
    [{"event": "collection", "tests": ["test.py::test_x"]},
     {"event": "phase", "test": "test.py::test_x", "when": "teardown", "outcome": "passed",
      "assertion": False, "missing": None}],
])
def test_report_rejects_invalid_population_and_phase_order(middle):
    records = [{"event": "start", "schema": 1}, *middle, {"event": "finish", "exit_code": 0}]
    with pytest.raises(InvalidReport):
        parse(records, ("test.py::test_x",), set(), 0)


def test_config_edits_are_rejected(trees):
    base, head, _ = trees
    (head / "conftest.py").write_text("def pytest_collection_modifyitems(items): items.clear()\n")
    assert changed_configuration(base, head) == ["conftest.py"]


def test_export_uses_commit_not_dirty_files_or_archive_attributes(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    git(repo, "init", "-q")
    (repo / "product.py").write_text("original\n")
    (repo / ".gitattributes").write_text("product.py export-ignore\n")
    git(repo, "add", ".")
    git(repo, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", "base")
    (repo / "product.py").write_text("dirty\n")
    target = tmp_path / "export"
    export(repo, "HEAD", target)
    assert (target / "product.py").read_text() == "original\n"


def test_symlink_export_is_rejected(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    git(repo, "init", "-q")
    (repo / "secret").symlink_to("/host-sentinel")
    git(repo, "add", ".")
    git(repo, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", "link")
    with pytest.raises(ValueError, match="regular files"):
        export(repo, "HEAD", tmp_path / "export")


def test_failure_message_survives_real_pytest_and_judging(trees):
    _, head, _ = trees
    test = 'tests/test_product.py::test_regression_breaks'
    child = subprocess.run([sys.executable, '-I', '-B', str(RUNNER), json.dumps([test]), str(head)],
                           capture_output=True)
    records = [decode(line) for line in child.stdout.splitlines()]
    messages = {}
    outcomes = parse(records, (test,), set(), child.returncode, messages=messages)
    assert outcomes[test] == Outcome.ASSERTION
    assert 'assert' in messages[test]
    result = judge(Check('regression', Kind.REGRESSION, (test,)),
                   {test: Outcome.PASSED}, outcomes, messages=messages)
    assert result.tests[0].message == messages[test]
    assert len(result.tests[0].message) <= 1000


def test_failure_messages_are_bounded_without_changing_verdicts():
    test = 'test.py::test_x'
    result = judge(Check('bounded', Kind.REGRESSION, (test,)),
                   {test: Outcome.PASSED}, {test: Outcome.ASSERTION}, messages={test: 'é' * 2000})
    assert result.verdict == Verdict.FAIL
    assert result.tests[0].message == 'é' * 1000
    legacy = judge(Check('legacy', Kind.REGRESSION, (test,)),
                   {test: Outcome.PASSED}, {test: Outcome.ASSERTION})
    assert 'head assertion' in legacy.tests[0].message


@pytest.mark.parametrize("name,expected", [
    ("test_demonstrates", Verdict.PASS),
    ("test_does_not_demonstrate", Verdict.FAIL),
    ("test_setup", Verdict.FAIL),
    ("test_runtime_error", Verdict.FAIL),
    ("test_new_name", Verdict.PASS),
    ("test_skipped", Verdict.FAIL),
    ("test_xfailed", Verdict.FAIL),
    ("uncollected", Verdict.FAIL),
    ("test_absent_name", Verdict.FAIL),
])
def test_reproducing_verdicts(trees, name, expected):
    kind = next((kind for kind in Kind if kind.value == "reproducing"), None)
    assert kind is not None, "the harness must judge reproducing checks"
    reviewed, fixed, reproducing = trees
    notes = FailureNotes('old product value')
    tests, before = run(reproducing, [name], added_names(reviewed, fixed, ["tests/*.py"]), notes=notes)
    _, after = run(fixed, [name])
    result = judge(Check(name, kind, tests, proof=Proof((), (), "old product value") if kind in (Kind.DEMONSTRATING, Kind.REPRODUCING) else None), before, after, base_notes=notes)
    assert result.verdict == expected, asdict(result)
    assert not result.tests[0].pre_existing


@pytest.mark.parametrize("before,after", [
    (Outcome.MISSING, Outcome.PASSED), (Outcome.PASSED, Outcome.MISSING),
    (Outcome.ASSERTION, Outcome.ASSERTION), (Outcome.ASSERTION, Outcome.SKIPPED),
    (Outcome.ASSERTION, Outcome.UNCOLLECTED), (Outcome.ERROR, Outcome.PASSED),
])
def test_reproducing_requires_available_tests_and_a_passing_fix(before, after):
    kind = next((kind for kind in Kind if kind.value == "reproducing"), None)
    assert kind is not None, "reproducing checks must reject unavailable or broken tests"
    test = "tests/test_product.py::test_demonstrates"
    assert judge(Check("fix", kind, (test,)), {test: before}, {test: after}).verdict == Verdict.FAIL


@pytest.mark.parametrize("cpus", [None, "4"])
def test_container_preserves_admitted_native_thread_bounds(tmp_path, monkeypatch, cpus):
    names = ("OMP_NUM_THREADS", "MKL_NUM_THREADS", "OPENBLAS_NUM_THREADS")
    for name in names:
        monkeypatch.delenv(name, raising=False)
        if cpus is not None:
            monkeypatch.setenv(name, cpus)
    monkeypatch.setattr("tl3_harness.execution.scratch_directory", lambda: tmp_path)
    command = container_command(tmp_path, Container(IMAGE), ())
    for name in names:
        assert f"{name}={cpus or '1'}" in command


def test_added_names_include_new_module_constants(tmp_path):
    # PQ #2662: a new module-level constant imported in a test body is a name the change adds.
    base, head = tmp_path / "base", tmp_path / "head"
    for tree in (base, head):
        (tree / "pkg").mkdir(parents=True)
    (base / "pkg/digests.py").write_text("OLD = 1\n")
    (head / "pkg/digests.py").write_text(
        "OLD = 1\n"
        "NEW_PROFILE = dict(a=1)\n"
        "TYPED: int = 2\n"
        "BARE: int\n"
        "LEFT = RIGHT = 3\n"
        "FIRST, (SECOND, *REST) = 1, (2, 3)\n"
        "obj = object()\n"
        "obj.attribute = 4\n"
        "table = {}\n"
        "table['key'] = 5\n"
        "if True:\n"
        "    CONDITIONAL = 6\n")
    names = {name for path, name in added_names(base, head, ["tests/*.py"]) if path == "pkg/digests.py"}
    assert {"NEW_PROFILE", "TYPED", "LEFT", "RIGHT", "FIRST", "SECOND", "REST", "obj", "table"} <= names
    # Attribute and subscript stores bind no module name; nested statements are not module-level bindings.
    assert not names & {"attribute", "key", "CONDITIONAL", "OLD", "BARE"}

