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
