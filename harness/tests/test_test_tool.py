import json
from pathlib import Path
import sys

import pytest

from tl3_harness import execution
from tl3_harness import test_tool


@pytest.fixture
def fixture(tmp_path, monkeypatch):
    (tmp_path / "test_value.py").write_text('''
import pytest

def test_pass():
    print("x" * 1000000)
    assert True

def test_fail():
    assert False, "x" * 10000

def test_error():
    raise RuntimeError("fixture error")

@pytest.mark.skip(reason="fixture")
def test_skip():
    pass

@pytest.fixture
def broken():
    raise ValueError("broken setup")

def test_setup(broken):
    pass
''')
    monkeypatch.setattr(test_tool, "require_admission", lambda: None)
    monkeypatch.setenv("PRISMABUILD_ACTION_PROGRESS_PATH", str(tmp_path / "progress.json"))
    monkeypatch.setenv("PRISMABUILD_ACTION_PROGRESS_TOKEN", "fixture")
    monkeypatch.setattr(test_tool, "container_command", lambda tree, container, tests: [
        sys.executable, "-I", "-B", str(Path(execution.__file__).with_name("runner.py")),
        json.dumps(tests), str(tree),
    ])
    return tmp_path


def test_whole_suite_reports_each_outcome_and_progress(fixture):
    result = test_tool.run_current(fixture, execution.Container("sha256:" + "1" * 64), ())
    assert result["status"] == "completed"
    assert result["totals"] == {"passed": 1, "failed": 1, "error": 2, "skipped": 1}
    assert len(result["tests"]) == 5
    assert "Truncated" in result["note"]
    assert json.loads((fixture / "progress.json").read_text())["units_completed"] == 5


@pytest.mark.parametrize("selectors,passed", [(('test_value.py::test_pass',), 1), (('test_value.py',), 1)])
def test_named_node_and_file(fixture, selectors, passed):
    result = test_tool.run_current(fixture, execution.Container("sha256:" + "1" * 64), selectors)
    assert result["status"] == "completed"
    assert result["totals"]["passed"] == passed
    if "::" in selectors[0]:
        assert sum(result["totals"].values()) == 1


def test_collection_error(fixture):
    (fixture / "test_value.py").write_text('raise RuntimeError("collection fixture")\n')
    result = test_tool.run_current(fixture, execution.Container("sha256:" + "1" * 64), ())
    assert result["status"] == "completed"
    assert result["totals"]["error"] == 1
    assert "collection fixture" in result["tests"][0]["message"]


def test_missing_files_are_reported_before_launch(fixture, monkeypatch):
    def unexpected_launch(*args):
        pytest.fail("missing selectors must be checked before launching pytest")

    monkeypatch.setattr(test_tool, "container_command", unexpected_launch)
    selectors = ("missing.py", "other.py::test_absent")
    result = test_tool.run_current(fixture, execution.Container("sha256:" + "1" * 64), selectors)
    assert result["status"] == "completed"
    assert result["totals"] == {"passed": 0, "failed": 0, "error": 2, "skipped": 0}
    assert {item["test"] for item in result["tests"]} == set(selectors)
    assert all("selector not found" in item["message"].lower() for item in result["tests"])


def test_missing_node_is_selector_error(fixture):
    selectors = ("test_value.py::test_absent", "test_value.py::test_also_absent")
    result = test_tool.run_current(fixture, execution.Container("sha256:" + "1" * 64), selectors)
    assert result["status"] == "completed"
    assert result["totals"] == {"passed": 0, "failed": 0, "error": 2, "skipped": 0}
    assert {item["test"] for item in result["tests"]} == set(selectors)
    assert all("selector not found" in item["message"].lower() for item in result["tests"])


def test_directory_selector(fixture):
    tests = fixture / "tests"
    tests.mkdir()
    (tests / "test_nested.py").write_text("def test_nested():\n    assert True\n")
    result = test_tool.run_current(fixture, execution.Container("sha256:" + "1" * 64), ("tests",))
    assert result["status"] == "completed"
    assert result["totals"] == {"passed": 1, "failed": 0, "error": 0, "skipped": 0}


def test_no_report_is_infrastructure(fixture):
    (fixture / "test_value.py").write_text('import os\nos._exit(0)\n')
    result = test_tool.run_current(fixture, execution.Container("sha256:" + "1" * 64), ())
    assert result["status"] == "infrastructure_error"
    assert sum(result["totals"].values()) == 0


def test_summary_accepts_node_ids_above_the_old_line_cap(fixture, monkeypatch):
    test = 'test_value.py::test_' + 'x' * 70000
    script = fixture / 'summary.py'
    script.write_text('import json\n'
                      'print(json.dumps({"event":"start","schema":1}))\n'
                      f'print(json.dumps({{"event":"result","test":{test!r},"outcome":"passed","message":"","truncated":False}}))\n'
                      'print(json.dumps({"event":"finish","exit_code":0}))\n')
    monkeypatch.setattr(test_tool, 'container_command', lambda *args: [sys.executable, str(script)])
    result = test_tool.run_current(fixture, execution.Container('sha256:' + '1' * 64), (test,))
    assert result['status'] == 'completed'
    assert result['totals']['passed'] == 1


@pytest.mark.parametrize("files,selectors", [
    ({"test_value.py": "import os\n\ndef test_exit():\n    os._exit(3)\n"}, ()),
    ({"conftest.py": "import os\nos._exit(3)\n"}, ()),
    # Docker's own container-failure codes, from the checkout's code before pytest starts.
    *[({"conftest.py": f"import os\nos._exit({code})\n"}, ()) for code in (125, 126, 127)],
    ({"conftest.py": "import sys\nsys.exit(127)\n"}, ()),
    ({"tests/conftest.py": "import os\nos._exit(126)\n",
      "tests/test_value.py": "def test_ok():\n    pass\n"}, ("tests/test_value.py",)),
    *[({"pytest.ini": "[pytest]\naddopts = -p hostile\n",
        "hostile.py": f"import os\nos._exit({code})\n"}, ()) for code in (125, 126, 127)],
])
def test_report_cut_short_by_the_code_under_test_is_marked(fixture, files, selectors):
    # Q50: an incomplete report the tested code could cause is not citable environment evidence.
    for path, source in files.items():
        (fixture / path).parent.mkdir(exist_ok=True)
        (fixture / path).write_text(source)
    result = test_tool.run_current(fixture, execution.Container("sha256:" + "1" * 64), selectors)
    assert result["status"] == "infrastructure_error"
    assert result["origin"] == "test_run"


def test_container_that_cannot_start_is_harness_infrastructure(fixture, monkeypatch):
    monkeypatch.setattr(test_tool, "container_command", lambda tree, container, tests: [
        sys.executable, "-I", "-c", "import sys; sys.exit(125)"])
    result = test_tool.run_current(fixture, execution.Container("sha256:" + "1" * 64), ())
    assert result["status"] == "infrastructure_error"
    assert "origin" not in result


CONTAINER = execution.Container("sha256:" + "1" * 64)
# Each pytest process appends its PID once, and each test its xdist worker.
PROCESS_PROBE = '''
import os
from pathlib import Path

def pytest_configure(config):
    if not os.environ.get("PYTEST_XDIST_WORKER"):
        with Path(__file__).with_name("processes.log").open("a") as log:
            log.write(f"{os.getpid()}\\n")
'''
WORKER_PROBE = '''
import os
from pathlib import Path

def test_worker_one():
    with Path(__file__).with_name("workers.log").open("a") as log:
        log.write(os.environ.get("PYTEST_XDIST_WORKER", "none") + "\\n")

def test_worker_two():
    with Path(__file__).with_name("workers.log").open("a") as log:
        log.write(os.environ.get("PYTEST_XDIST_WORKER", "none") + "\\n")
'''


def test_repeat_runs_fresh_pytest_processes_and_reports_each(fixture):
    # #408: five fresh consecutive runs, each its own pytest process.
    (fixture / "conftest.py").write_text(PROCESS_PROBE)
    result = test_tool.run(fixture, CONTAINER, ("test_value.py::test_pass",), repeat=5)
    processes = (fixture / "processes.log").read_text().split()
    assert len(processes) == 5 and len(set(processes)) == 5
    assert result["status"] == "completed"
    assert result["repeat"] == 5
    assert [run["run"] for run in result["runs"]] == [1, 2, 3, 4, 5]
    assert all(run["status"] == "completed" and run["totals"]["passed"] == 1 for run in result["runs"])
    assert result["totals"] == {"passed": 1, "failed": 0, "error": 0, "skipped": 0}
    assert json.loads((fixture / "progress.json").read_text())["units_completed"] == 5


def test_repeat_keeps_any_failing_run(fixture):
    (fixture / "test_flaky.py").write_text(
        "from pathlib import Path\n\n"
        "def test_second_run_fails():\n"
        "    counter = Path(__file__).with_name('count')\n"
        "    run = int(counter.read_text()) + 1 if counter.exists() else 1\n"
        "    counter.write_text(str(run))\n"
        "    assert run != 2\n")
    result = test_tool.run(fixture, CONTAINER, ("test_flaky.py",), repeat=3)
    assert [run["totals"]["failed"] for run in result["runs"]] == [0, 1, 0]
    assert result["totals"]["failed"] == 1, "a test that failed in any run is reported failed"
    assert result["runs"][1]["failed"] == ["test_flaky.py::test_second_run_fails"]


def test_parallel_runs_through_xdist(fixture):
    (fixture / "test_workers.py").write_text(WORKER_PROBE)
    result = test_tool.run(fixture, CONTAINER, ("test_workers.py",), parallel=2)
    assert result["status"] == "completed"
    assert result["parallel"] == 2
    assert result["totals"] == {"passed": 2, "failed": 0, "error": 0, "skipped": 0}
    workers = set((fixture / "workers.log").read_text().split())
    assert workers and workers <= {"gw0", "gw1"}, workers
    # Outcomes keep their classes under xdist.
    result = test_tool.run(fixture, CONTAINER, ("test_value.py",), parallel=2)
    assert result["totals"] == {"passed": 1, "failed": 1, "error": 2, "skipped": 1}


# PB #2710 (#799): the untouched base exhausted the container's pid limit, so
# xdist could not start the thread that stops its workers. That happens in
# pytest_sessionfinish, after the last result. Fail at the same call.
TEARDOWN_EXHAUSTED = '''
import execnet.multi

def pytest_configure(config):
    if not hasattr(config, "workerinput"):
        def exhausted(*args, **kwargs):
            raise RuntimeError("can't start new thread")
        execnet.multi.safe_terminate = exhausted
'''


@pytest.mark.parametrize("module,totals", [
    ("test_value.py", {"passed": 1, "failed": 1, "error": 2, "skipped": 1}),
    ("test_workers.py", {"passed": 2, "failed": 0, "error": 0, "skipped": 0}),
    ("test_broken.py", {"passed": 0, "failed": 0, "error": 1, "skipped": 0}),
])
def test_contention_run_reports_outcomes_when_worker_teardown_fails(fixture, module, totals):
    (fixture / "test_workers.py").write_text(WORKER_PROBE)
    (fixture / "test_broken.py").write_text('raise RuntimeError("collection fixture")\n')
    (fixture / "conftest.py").write_text(TEARDOWN_EXHAUSTED)
    result = test_tool.run(fixture, CONTAINER, (module,), repeat=2, parallel=2)
    assert result["status"] == "completed", result.get("message")
    assert "execution" not in result
    assert result["totals"] == totals
    assert [run["totals"] for run in result["runs"]] == [totals, totals]


def test_truncated_contention_report_stays_incomplete(fixture):
    # The controller dies after its first result, before any finish record.
    (fixture / "conftest.py").write_text('''
import os

CONTROLLER = []

def pytest_configure(config):
    if not hasattr(config, "workerinput"):
        CONTROLLER.append(config)

def pytest_runtest_logreport(report):
    if CONTROLLER and report.when == "teardown":
        CONTROLLER.append(report)
        if len(CONTROLLER) > 2:
            os._exit(1)
''')
    result = test_tool.run(fixture, CONTAINER, ("test_value.py",), repeat=2, parallel=2)
    assert result["status"] == "infrastructure_error", result
    assert result["execution"]["outcome"] == "unknown_incomplete"
    assert result["execution"]["termination_reason"] == "incomplete_report"
    assert result["message"].startswith("Run 1 of 2:")
    assert result["runs"] == []


def test_parallel_without_xdist_is_named_not_available(fixture, monkeypatch):
    runner = Path(execution.__file__).with_name("runner.py")
    monkeypatch.setattr(test_tool, "container_command", lambda tree, container, tests: [
        sys.executable, "-I", "-B", "-c",
        "import runpy, sys; sys.modules['xdist'] = None; "
        f"sys.argv = [{str(runner)!r}, {json.dumps(tests)!r}, {str(fixture)!r}] + sys.argv[1:]; "
        f"runpy.run_path({str(runner)!r}, run_name='__main__')",
    ])
    result = test_tool.run(fixture, CONTAINER, ("test_value.py",), parallel=2)
    assert result["status"] == "not_available"
    assert "pytest-xdist" in result["message"]
    assert sum(result["totals"].values()) == 0


@pytest.mark.parametrize("repeat,parallel", [(0, 0), (11, 0), (1, 1), (1, 5), (True, 0)])
def test_options_are_bounded(fixture, repeat, parallel):
    with pytest.raises(ValueError):
        test_tool.run(fixture, CONTAINER, (), repeat=repeat, parallel=parallel)


def test_repeat_with_many_long_failures_stays_within_the_summary_bound(fixture):
    # #432: worker fields (receipts, tree, action key) still fit after the harness result.
    (fixture / "test_long.py").write_text("".join(
        f"def test_{'x' * 400}_{index}():\n    assert False, {'y' * 2000!r}\n" for index in range(40)))
    result = test_tool.run(fixture, CONTAINER, ("test_long.py",), repeat=3)
    assert result["status"] == "completed"
    assert result["totals"]["failed"] == 40
    assert [run["totals"]["failed"] for run in result["runs"]] == [40, 40, 40]
    assert len(json.dumps(result).encode()) <= test_tool.SUMMARY_BYTES - 1024
    assert result["note"].startswith("Truncated")


@pytest.mark.parametrize('hang', ['body', 'cleanup', 'descendant', 'session_cleanup'])
def test_unproven_hang_stays_unknown_with_bounded_cleanup(fixture, monkeypatch, hang):
    import time
    monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_ALLOWANCES', '{"tests": 3}')
    source = '''import signal, time, subprocess, sys
signal.alarm(8)
def test_hang():
    while True:
        time.sleep(1)
'''
    if hang == 'cleanup':
        source = '''import signal, time, pytest
signal.alarm(8)
@pytest.fixture
def cleanup():
    yield
    while True:
        time.sleep(1)
def test_hang(cleanup):
    assert True
'''
    if hang == 'descendant':
        source = '''import signal, subprocess, sys
signal.alarm(8)
def test_hang():
    child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(8)'],
                             stdout=subprocess.PIPE)
    child.communicate()
'''
    if hang == 'session_cleanup':
        (fixture / 'conftest.py').write_text('''import signal, time
signal.alarm(8)
def pytest_unconfigure(config):
    while True:
        time.sleep(1)
''')
        source = 'def test_hang():\n    assert True\n'
    (fixture / 'test_value.py').write_text(source)
    started = time.monotonic()
    result = test_tool.run_current(fixture, CONTAINER, ('test_value.py::test_hang',))
    assert result['status'] == 'infrastructure_error', result
    assert sum(result['totals'].values()) == 0, result
    assert result['execution']['outcome'] == 'unknown_incomplete'
    assert result['execution']['phase'] == 'unknown'
    assert result['execution']['termination_reason'] == 'no_progress'
    assert result['execution']['test_started'] is False
    assert result['origin'] == 'test_run'
    assert result['selectors'] == ['test_value.py::test_hang']
    assert time.monotonic() - started < 7, 'supervision must finish before the outer allowance'


def test_pretest_hang_is_setup_infrastructure(fixture, monkeypatch):
    monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_ALLOWANCES', '{"tests": 3}')
    (fixture / 'conftest.py').write_text('import signal, time\nsignal.alarm(8)\nwhile True: time.sleep(1)\n')
    result = test_tool.run_current(fixture, CONTAINER, ('test_value.py::test_pass',))
    assert result['status'] == 'infrastructure_error'
    assert result.get('execution', {}).get('outcome') == 'unknown_incomplete', result
    assert result['execution']['test_started'] is False


def test_real_missing_container_runtime_stays_infrastructure(fixture, monkeypatch):
    # Invoke the real container launcher with a PATH that cannot contain Docker.
    # No test process can start, even on a worker that happens to have a daemon.
    monkeypatch.setenv('PATH', str(fixture / 'no-container-runtime'))
    monkeypatch.setattr(execution, 'scratch_directory', lambda: fixture)
    monkeypatch.setattr(test_tool, 'container_command', execution.container_command)
    try:
        result = test_tool.run_current(fixture, CONTAINER, ('test_value.py::test_pass',))
    except OSError:
        result = {'status': 'uncaught_container_start_failure'}
    assert result['status'] == 'infrastructure_error', result
    assert result['execution']['outcome'] == 'setup_failure'
    assert result['execution']['phase'] == 'container_start'
    assert result['execution']['test_started'] is False


def test_incomplete_report_has_explicit_unknown_ownership(fixture):
    (fixture / 'test_value.py').write_text('import os\nos._exit(0)\n')
    result = test_tool.run_current(fixture, CONTAINER, ())
    assert result['status'] == 'infrastructure_error'
    assert result.get('execution', {}).get('outcome') == 'unknown_incomplete', result
    assert result['origin'] == 'test_run'


@pytest.mark.parametrize('kind', ['demonstrating', 'regression', 'behavior-preserving'])
def test_independent_checking_unproven_hang_never_supplies_assertion_red(fixture, monkeypatch, kind):
    from tl3_harness.model import Check, Kind, Outcome, Verdict, judge
    monkeypatch.setattr(execution, 'container_command', test_tool.container_command)
    monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_ALLOWANCES', '{"tests": 3}')
    (fixture / 'test_value.py').write_text('import signal, time\nsignal.alarm(8)\ndef test_hang():\n    while True: time.sleep(1)\n')
    tests = ('test_value.py::test_hang',)
    messages = {}
    outcomes = execution.run_tree(fixture, CONTAINER, tests, fixture / 'report.jsonl',
                                  execution.Progress(), messages=messages)
    result = judge(Check('hang', Kind(kind), tests), outcomes, outcomes, messages=messages)
    assert result.verdict == Verdict.INFRASTRUCTURE_FAILURE, result
    assert outcomes[tests[0]] == Outcome.NO_REPORT
    assert result.tests[0].passed is False
    assert 'timeout' in result.tests[0].message.lower()


@pytest.mark.parametrize('access', ['plugin_manager', 'gc'])
def test_forged_test_start_during_collection_cannot_prove_owned_timeout(fixture, monkeypatch, access):
    monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_ALLOWANCES', '{"tests": 3}')
    if access == 'plugin_manager':
        (fixture / 'conftest.py').write_text('''import signal, time
signal.alarm(8)
def pytest_configure(config):
    reporter = next(plugin for plugin in config.pluginmanager.get_plugins()
                    if type(plugin).__name__ == 'Reporter')
    reporter.emit(event='test-start', test='test_value.py::test_pass')
    while True:
        time.sleep(1)
''')
    else:
        (fixture / 'test_value.py').write_text('''import gc, signal, time
signal.alarm(8)
reporter = next(obj for obj in gc.get_objects() if type(obj).__name__ == 'Reporter')
reporter.emit(event='test-start', test='test_value.py::test_pass')
while True:
    time.sleep(1)
def test_pass():
    assert True
''')
    result = test_tool.run_current(fixture, CONTAINER, ('test_value.py::test_pass',))
    assert result['status'] == 'infrastructure_error', result
    assert result['execution']['outcome'] == 'unknown_incomplete', result
    assert result['execution']['test_started'] is False
    assert sum(result['totals'].values()) == 0


@pytest.mark.parametrize('report_pipe', ['open', 'closed'])
def test_clean_runner_exit_followed_by_container_wait_stall_is_infrastructure(
        fixture, monkeypatch, tmp_path, report_pipe):
    import time
    monkeypatch.setenv('PRISMABUILD_ACTION_PROGRESS_ALLOWANCES', '{"tests": 3}')
    (fixture / 'test_value.py').write_text('def test_pass():\n    assert True\n')
    runner = Path(execution.__file__).with_name('runner.py')
    # This wrapper is outside the pytest process. Its wait() observes the actual
    # runner exit before it simulates a hung container attach/wait operation.
    wrapper = tmp_path / 'container_wait.py'
    exited = tmp_path / 'runner-exited.json'
    wrapper.write_text('''import json, os, signal, subprocess, sys, time
from pathlib import Path
signal.alarm(8)
command = [sys.executable, '-I', '-B', sys.argv[1], sys.argv[2], sys.argv[3], *sys.argv[6:]]
with subprocess.Popen(command, stdout=subprocess.PIPE) as runner:
    while chunk := os.read(runner.stdout.fileno(), 65536):
        os.write(1, chunk)
    code = runner.wait()
Path(sys.argv[4]).write_text(json.dumps({'runner_exit_code': code}))
if sys.argv[5] == 'closed':
    os.close(1)
while True:
    time.sleep(1)
''')
    monkeypatch.setattr(test_tool, 'container_command', lambda tree, container, tests: [
        sys.executable, '-I', '-B', str(wrapper), str(runner), json.dumps(tests), str(tree),
        str(exited), report_pipe,
    ])
    started = time.monotonic()
    result = test_tool.run_current(fixture, CONTAINER, ('test_value.py::test_pass',))
    assert json.loads(exited.read_text()) == {'runner_exit_code': 0}, 'the real runner must have exited'
    assert result['status'] == 'infrastructure_error', result
    assert result['execution']['outcome'] in ('transport_failure', 'unknown_incomplete'), result
    assert result['execution']['test_started'] is False
    assert sum(result['totals'].values()) == 0
    assert time.monotonic() - started < 7, 'transport cleanup must remain bounded'
