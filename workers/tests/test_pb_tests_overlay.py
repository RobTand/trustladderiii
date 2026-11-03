# ruff: noqa: F811

from pathlib import Path
import pytest
from tl3_workers.pb_tests import TestSettings, make_test_tool
from tl3_workers.tools import Worktree

from worker_process import IMAGE
from worker_process import generator_backend  # noqa: F401
from worker_process import generator_repo  # noqa: F401
from worker_process import prepare_base_submission
from worker_process import submission  # noqa: F401


@pytest.mark.parametrize("case,expected", [
    ("edited", "pass"), ("unchanged", "fail"), ("broken_head", "fail"),
    ("import_error", "fail"), ("setup_error", "fail"), ("missing_selector", "fail"),
    ("configuration", "fail"), ("protected", "fail"), ("helper_only", "fail"),
])
def test_request_158_plain_base_is_advisory_and_checking_owns_overlay(
        submission, tmp_path, monkeypatch, case, expected):
    import sys
    from tl3_harness import execution
    from tl3_harness.model import Check, Kind, Outcome, Proof
    from tl3_workers.pb_tests import _git

    root, scripts = submission
    (root / "pyproject.toml").write_text(
        '[tool.tl3.harness]\ncommand=["python3", "-m", "pytest"]\ntest-files=["test_*.py"]\n')
    base, _, _, git = prepare_base_submission(submission, base_value="head", head_value="fixed")
    test = "test_code.py::test_value"
    source = (root / "test_code.py").read_text()
    if case == "unchanged":
        (root / "example_module.py").write_text("value = 'head'\n")
    if case != "unchanged":
        source = source.replace("assert value == 'head'", "assert value in ('head', 'fixed')\n    assert value == 'fixed', 'fixed value'", 1)
    if case == "broken_head":
        (root / "example_module.py").write_text("value = 'broken'\n")
    elif case == "import_error":
        source = "import nonexistent_request_158_module\n" + source
    elif case == "setup_error":
        source = "import pytest\n@pytest.fixture(autouse=True)\ndef setup(): assert False\n" + source
    elif case == "missing_selector":
        test = "test_code.py::test_absent"
    elif case == "configuration":
        (root / "pytest.ini").write_text("[pytest]\n")
    elif case == "helper_only":
        # Whole-file overlay carries the helper fix too. It cannot manufacture RED.
        (root / "example_module.py").write_text("value = 'head'\n")
        source = "def helper(): return 'fixed'\ndef test_value(): assert helper() == 'fixed'\n"
    (root / "test_code.py").write_text(source)
    with Worktree(root) as tree:
        tree.test_checkout, tree.check_base_sha = root, base
        tool = make_test_tool(tree, TestSettings(IMAGE, scripts))
        baseline = tool([test], base=True)
        if case == "edited":
            (root / "example_module.py").write_text("value = 'head'\n")
            red = tool([test])
            assert red["totals"]["failed"] == 1, red
            assert "assert value == 'fixed'" in red["tests"][0]["message"]
            assert red["tests"][0]["base_outcome"] == "passed"
            (root / "example_module.py").write_text("value = 'fixed'\n")
        current = tool([test])
    git("add", ".")
    git("commit", "--quiet", "-m", "Request 158 edited existing test")
    head = git("rev-parse", "HEAD")
    # Only the container transport is replaced: real staging, selection, runner,
    # report parsing and judging all execute inside the outer admitted PB job.
    runner = Path(execution.__file__).with_name("runner.py")
    monkeypatch.setattr(execution, "require_admission", lambda: None)
    monkeypatch.setenv("PRISMABUILD_CONTAINER_OWNER", str(tmp_path))
    monkeypatch.setenv("PRISMABUILD_ACTION_PROGRESS_PATH", str(tmp_path / "progress.json"))
    monkeypatch.setenv("PRISMABUILD_ACTION_PROGRESS_TOKEN", "fixture-token")
    observed = {}
    def command(staged, container, tests):
        observed[staged.name] = _git(root, "hash-object", str(staged / "test_code.py")).strip()
        return [sys.executable, "-I", "-B", str(runner), "-", str(staged)]
    monkeypatch.setattr(execution, "container_command", command)
    [result] = execution.run_checks(
        root, base, head, (Check("fallback", Kind.DEMONSTRATING, (test,), proof=Proof((), (), 'fixed value')),),
        container=execution.Container(IMAGE), protected=("example_module.py",) if case == "protected" else ())
    assert result.verdict == expected, result
    if case == "edited":
        assert baseline["totals"]["passed"] == 1, baseline
        assert current["totals"]["passed"] == 1, current
        assert result.tests[0].base == Outcome.ASSERTION
        assert result.tests[0].head == Outcome.PASSED
        assert observed["proof-0"] == observed["head"]
    assert baseline["base_semantics"] == "untouched_base"
    assert baseline["demonstrating_proof"] == "pending_checking"
