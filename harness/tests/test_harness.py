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


def source_guard_run(tmp_path, body, *, proof=True, setup='', teardown='', subject=None, summary=False, parallel=0):
    (tmp_path / 'app.py').write_text(subject or 'def value():\n    return 1\n')
    (tmp_path / 'support.py').write_text(
        'def source(path):\n    return path.read_bytes()\n')
    tests = tmp_path / 'tests'
    tests.mkdir()
    (tests / 'test_product.py').write_text(f'''import pytest
from pathlib import Path
@pytest.fixture(autouse=True)
def context():
    import app
{setup or '    pass'}
    yield
{teardown or '    pass'}
def test_guard():
    import app
    path = Path(app.__file__)
{body}
def test_next():
    import app
    assert app.value() == 1
''')
    nodes = ('tests/test_product.py::test_guard', 'tests/test_product.py::test_next')
    request = {'tests': nodes}
    if proof:
        request['proof_subjects'] = {nodes[0]: ['app.py'], nodes[1]: ['app.py']}
    result = subprocess.run([sys.executable, '-I', '-B', str(RUNNER), json.dumps(request),
                             str(tmp_path), *(['--summary'] if summary else []),
                             *([f'--parallel={parallel}'] if parallel else [])],
                            capture_output=True, env={
                                'PATH': os.environ['PATH'], 'PYTEST_DISABLE_PLUGIN_AUTOLOAD': '1'})
    records = [decode(line) for line in result.stdout.splitlines()]
    return nodes, records, result.returncode


@pytest.mark.parametrize('body,setup', [
    ('    from support import source\n    source(path)', ''),
    ('    open(app.__cached__, "rb").read()',
     '    import py_compile\n    py_compile.compile(app.__file__, doraise=True)'),
    ('    import mmap, os\n    fd = os.open(path, os.O_RDONLY)\n'
     '    with mmap.mmap(fd, 0, access=mmap.ACCESS_READ) as data:\n        data[:]\n    os.close(fd)', ''),
    ('    import mmap, os\n    with mmap.mmap(app.fd, 0, access=mmap.ACCESS_READ) as data:\n        data[:]',
     '    import os\n    app.fd = os.open(app.__file__, os.O_RDONLY)'),
    ('    try:\n        path.read_text()\n    except Exception:\n        pass', ''),
    ('    import os\n    os.read(app.fd, 10000)',
     '    import os\n    app.fd = os.open(app.__file__, os.O_RDONLY)'),
    ('    import io\n    io.open(app.fd, "rb", closefd=False).read()',
     '    import os\n    app.fd = os.open(app.__file__, os.O_RDONLY)'),
])
def test_runner_source_guard_records_reads_and_resets_per_test(tmp_path, body, setup):
    nodes, records, exit_code = source_guard_run(tmp_path, body, setup=setup)
    notes = FailureNotes()
    outcomes = parse(records, nodes, set(), exit_code, messages=notes)
    assert outcomes == {nodes[0]: Outcome.ERROR, nodes[1]: Outcome.PASSED}, records
    assert 'subject_source_read' in notes[nodes[0]]
    assert ('support.py:2' if 'from support' in body else 'tests/test_product.py:') in notes[nodes[0]]
    assert '__pycache__/app.' in notes[nodes[0]] if '__cached__' in body else 'app.py' in notes[nodes[0]]


def test_external_bytecode_source_read_is_recorded(tmp_path):
    nodes, records, exit_code = source_guard_run(tmp_path,
        '    try:\n        open(app.__cached__, "rb").read()\n    except Exception:\n        pass',
        setup=('    import py_compile, sys, tempfile\n'
               '    app.cache = tempfile.TemporaryDirectory()\n'
               '    sys.pycache_prefix = app.cache.name\n'
               '    app.__cached__ = py_compile.compile(app.__file__, doraise=True)'),
        teardown='    app.cache.cleanup()')
    notes = FailureNotes()
    assert parse(records, nodes, set(), exit_code, messages=notes) == {
        nodes[0]: Outcome.ERROR, nodes[1]: Outcome.PASSED}, records
    assert 'subject_source_read' in notes[nodes[0]]


@pytest.mark.parametrize('case', ['import', 'self-source', 'setup-teardown', 'non-proof', 'write-only'])
def test_runner_source_guard_allows_subject_reads_and_other_phases(tmp_path, case):
    subject = ('from pathlib import Path\ndef value():\n'
               '    assert "def value" in Path(__file__).read_text()\n    return 1\n')
    nodes, records, exit_code = source_guard_run(tmp_path,
        '    path.read_bytes()' if case == 'non-proof' else (
            '    path.open("ab").close()' if case == 'write-only' else '    assert app.value() == 1'),
        proof=case != 'non-proof', subject=subject if case == 'self-source' else None,
        setup='    Path(app.__file__).read_bytes()' if case == 'setup-teardown' else '',
        teardown='    Path(app.__file__).read_text()' if case == 'setup-teardown' else '')
    assert parse(records, nodes, set(), exit_code) == dict.fromkeys(nodes, Outcome.PASSED), records


@pytest.mark.parametrize('parallel', [0, 2])
def test_summary_source_guard_uses_existing_error_result(tmp_path, parallel):
    nodes, records, exit_code = source_guard_run(tmp_path, '    path.read_bytes()', summary=True, parallel=parallel)
    results = [record for record in records if record['event'] == 'result']
    assert exit_code == 1, records
    assert {item['test']: item['outcome'] for item in results} == {nodes[0]: 'error', nodes[1]: 'passed'}
    assert 'subject_source_read' in next(item['message'] for item in results if item['test'] == nodes[0])


@pytest.mark.parametrize("kind,name,expected", [
    (Kind.DEMONSTRATING, "test_demonstrates", Verdict.PASS),
    (Kind.DEMONSTRATING, "test_new_name", Verdict.PASS),
    (Kind.REGRESSION, "test_pre_existing", Verdict.PASS),
    (Kind.BEHAVIOR_PRESERVING, "test_preserved", Verdict.PASS),
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


@pytest.mark.durability
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


def test_container_reaps_orphans(tmp_path, monkeypatch):
    monkeypatch.setattr('tl3_harness.execution.scratch_directory', lambda: tmp_path / 'scratch')
    command = container_command(tmp_path, Container(IMAGE), ('test_orphans.py::test_x',))
    assert '--init' in command[:command.index(IMAGE)]
    assert '--pids-limit=128' in command


def test_verification_suite_reaps_orphans(monkeypatch):
    import runpy

    verify = runpy.run_path(str(Path(__file__).parents[1] / 'tools/verify.py'))
    commands = []

    class CapturedSuite(Exception):
        pass

    def capture(command, **kwargs):
        commands.append(command)
        raise CapturedSuite

    monkeypatch.setitem(verify['main'].__globals__, 'require_admission', lambda: None)
    monkeypatch.setitem(verify['main'].__globals__, 'verify_orphans', lambda container: None)
    monkeypatch.setattr(sys, 'argv', ['verify.py', IMAGE])
    monkeypatch.setattr(subprocess, 'run', capture)
    with pytest.raises(CapturedSuite):
        verify['main']()
    assert '--init' in commands[0][:commands[0].index(IMAGE)]
    assert '--pids-limit=128' in commands[0]


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
    assert result.custody == Custody.IN_PROCESS
    assert not result.tests[0].pre_existing


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



def test_runner_siblings_load_without_the_tl3_harness_package(tmp_path):
    """#858: the container runs `python3 -I -B /harness/runner.py`, which loads
    its siblings by path. tl3_harness is not importable there, so a sibling that
    imports the package crashes the runner and every test reports no-report."""
    import re
    harness = Path(container_command.__code__.co_filename).parent
    siblings = re.findall(r'with_name\("(\w+\.py)"\)', (harness / "runner.py").read_text())
    assert {"node_ids.py", "forcing_ownership.py", "limits.py", "red_message.py", "source_reads.py"} <= set(siblings)
    mounted = tmp_path / "harness"
    shutil.copytree(harness, mounted, ignore=shutil.ignore_patterns("__pycache__"))
    for sibling in siblings:
        # -S as well as -I: a test interpreter's site hooks can expose tl3_harness,
        # which is how #858 passed every harness test. Prove the package is absent.
        code = ("import importlib.util\n"
                "assert importlib.util.find_spec('tl3_harness') is None, 'tl3_harness importable: test not isolated'\n"
                f"spec = importlib.util.spec_from_file_location('tl3_probe', {str(mounted / sibling)!r})\n"
                "module = importlib.util.module_from_spec(spec)\n"
                "spec.loader.exec_module(module)\n")
        result = subprocess.run([sys.executable, "-I", "-S", "-B", "-c", code], cwd=tmp_path,
                                capture_output=True, text=True)
        assert result.returncode == 0, (sibling, result.stderr[-2000:])
