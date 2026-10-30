from dataclasses import asdict
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

import pytest

from tl3_harness.execution import Container, Progress, container_command, run_checks
from tl3_harness.model import Check, Kind, Outcome, Verdict, judge
from tl3_harness.report import InvalidReport, decode, parse
from tl3_harness.trees import added_names, export, git, test_edits as apply_test_edits, validate_configuration


FIXTURES = Path(__file__).parent / "fixtures"
RUNNER = Path(__file__).parents[1] / "src/tl3_harness/runner.py"
IMAGE = "sha256:" + "1" * 64


@pytest.fixture
def trees(tmp_path):
    base, head, demo = [tmp_path / name for name in ("base", "head", "demo")]
    shutil.copytree(FIXTURES / "round/base", base)
    shutil.copytree(base, head)
    shutil.copytree(FIXTURES / "round/head", head, dirs_exist_ok=True)
    shutil.copytree(base, demo)
    apply_test_edits(base, head, demo, ["tests/*.py"])
    return base, head, demo


def run(tree, names, additions=frozenset()):
    tests = tuple("tests/test_product.py::" + name for name in names)
    env = {"PATH": os.environ["PATH"], "PYTEST_DISABLE_PLUGIN_AUTOLOAD": "1"}
    child = subprocess.run([sys.executable, "-I", "-B", str(RUNNER), json.dumps(tests), str(tree)],
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
    records = [decode(line) for line in child.stdout.splitlines()]
    missing = {test for test in tests if not (tree / test.split("::")[0]).exists()}
    return tests, parse(records, tests, missing, child.returncode, additions)


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
    tests, before = run(demo if kind == Kind.DEMONSTRATING else base, [name], additions)
    _, after = run(head, [name])
    result = judge(Check(name, kind, tests), before, after)
    assert result.verdict == expected, asdict(result)
    assert result.tests[0].pre_existing == (name == "test_pre_existing")


def test_only_test_edits_reach_demonstrating_base(trees):
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


def test_container_has_only_explicit_read_only_mounts_and_no_host_environment(tmp_path):
    command = container_command(tmp_path, Container(IMAGE), ("tests/test_x.py::test_x",))
    assert command[:2] == ["docker", "run"]
    for flag in ("--network=none", "--read-only", "--cap-drop=ALL", "--pull=never"):
        assert flag in command
    mounts = [command[index + 1] for index, value in enumerate(command) if value == "--mount"]
    assert len(mounts) == 2 and all(mount.endswith(",readonly") for mount in mounts)
    assert not any("/mnt/shared" in mount or "docker.sock" in mount for mount in mounts)
    assert "--cpuset-cpus" not in command


def test_requires_pb_admission(tmp_path, monkeypatch):
    monkeypatch.delenv("PRISMABUILD_CONTAINER_OWNER", raising=False)
    with pytest.raises(RuntimeError, match="admitted"):
        run_checks(tmp_path, "base", "head", (), container=Container(IMAGE))


@pytest.mark.parametrize("test", ["-k", "../test.py::test_x", "/test.py::test_x", "test.py", "test.py::"])
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


def test_config_edits_are_rejected(trees):
    base, head, _ = trees
    (head / "conftest.py").write_text("def pytest_collection_modifyitems(items): items.clear()\n")
    with pytest.raises(ValueError, match="configuration changed"):
        validate_configuration(base, head)


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
