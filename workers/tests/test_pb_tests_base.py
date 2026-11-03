# ruff: noqa: F811

import json
import subprocess
import pytest
from tl3_harness.test_tool import SUMMARY_BYTES, Summary
from tl3_workers.pb_tests import TestSettings, make_test_tool
from tl3_workers.tools import Worktree

from worker_process import IMAGE
from worker_process import KEY
from worker_process import call
from worker_process import generator_backend  # noqa: F401
from worker_process import generator_repo  # noqa: F401
from worker_process import prepare_base_submission
from worker_process import submission  # noqa: F401


def test_base_run_returns_base_outcomes_and_keeps_maker_tree(submission):
    root, scripts = submission
    base, head, before, git = prepare_base_submission(submission)
    with Worktree(root) as tree:
        tree.test_checkout = root
        tree.check_base_sha = base
        tool = make_test_tool(tree, TestSettings(IMAGE, scripts))
        current = tool(["test_code.py::test_value"])
        baseline = tool(["test_code.py::test_value"], base=True)
    assert current["totals"]["passed"] == 1, current
    assert baseline["totals"]["failed"] == 1, baseline
    assert baseline["tests"][0]["test"] == "test_code.py::test_value"
    assert baseline["action_key"] == "c" * 64
    assert baseline["base_sha"] == base
    assert git("rev-parse", "HEAD") == head
    assert git("status", "--porcelain") == before
    assert (root / "example_module.py").read_text() == "value = 'head'\n"


@pytest.mark.parametrize("base_value,head_value,classification,actions", [
    ("base", "broken", "pre-existing", 2),
    ("head", "broken", "new", 2),
    ("error", "error", "pre-existing", 2),
    ("head", "error", "new", 2),
    ("missing", "broken", "new", 2),
    ("base", "head", None, 1),
])
def test_automatic_base_comparison(submission, base_value, head_value, classification, actions):
    root, scripts = submission
    base, head, before, git = prepare_base_submission(submission, base_value, head_value)
    tests = ["test_code.py::test_value", "test_code.py::test_other", "test_code.py::test_pass"]
    with Worktree(root) as tree:
        tree.test_checkout = root
        tree.check_base_sha = base
        tree.test_checks = [{"kind": "regression", "tests": tests[:2]}]
        result = make_test_tool(tree, TestSettings(IMAGE, scripts))(tests)
    submitted = json.loads((scripts / "submissions.json").read_text())
    assert len(submitted) == actions, result
    assert result["action_key"] == KEY
    if classification:
        assert submitted[1] == tests[:2], "batch only failing tests into one base action"
        failed = [test for test in result["tests"] if test["test"] in tests[:2]]
        assert all(test["classification"] == classification for test in failed)
        assert result["base"]["action_key"] == "c" * 64
        assert result["base"]["base_sha"] == base
    else:
        assert "base" not in result
    assert git("rev-parse", "HEAD") == head
    assert git("status", "--porcelain") == before


@pytest.mark.parametrize("kind", ["regression", "demonstrating", "reproducing", "behavior-preserving", None])
@pytest.mark.parametrize("base_value", ["base", "error"])
def test_base_failure_classification_respects_check_kind(submission, kind, base_value):
    root, scripts = submission
    base, _, _, _ = prepare_base_submission(submission, base_value, "broken")
    test = "test_code.py::test_value"
    with Worktree(root) as tree:
        tree.test_checkout = root
        tree.check_base_sha = base
        tree.test_checks = [{"kind": kind, "tests": [test]}] if kind else []
        result = make_test_tool(tree, TestSettings(IMAGE, scripts))([test])
    assert result["tests"][0]["base_outcome"] == ("error" if base_value == "error" else "failed")
    assert result["tests"][0]["classification"] == (
        "pre-existing" if kind == "regression" else "fails-on-base"
    )
    assert len(json.loads((scripts / "submissions.json").read_text())) == 2


@pytest.mark.parametrize("selector", ["test_code.py::test_value", "test_code.py::test_value[0]"])
def test_regression_classification_matches_parametrized_selector(submission, selector):
    root, scripts = submission
    _, _, _, git = prepare_base_submission(submission)
    (root / "test_code.py").write_text(
        "import pytest\n@pytest.mark.parametrize('case', [0, 1])\n"
        "def test_value(case): assert False\n"
        "def test_value_extra(): assert False\n"
    )
    git("add", "--all")
    git("commit", "--quiet", "-m", "Parametrized check base")
    with Worktree(root) as tree:
        tree.test_checkout = root
        tree.check_base_sha = git("rev-parse", "HEAD")
        tree.test_checks = [{"kind": "regression", "tests": [selector]}]
        result = make_test_tool(tree, TestSettings(IMAGE, scripts))(["test_code.py"])
    assert {item["test"]: item["classification"] for item in result["tests"]} == {
        "test_code.py::test_value[0]": "pre-existing",
        "test_code.py::test_value[1]": "pre-existing" if selector.endswith("test_value") else "fails-on-base",
        "test_code.py::test_value_extra": "fails-on-base",
    }


def test_comparison_truncation_keeps_failures_before_passes(submission):
    summary = Summary()
    for i in range(60):
        summary.add(f"test_code.py::test_pass_{i}", "passed", "p" * 90)
    for i in range(30):
        summary.add(f"test_code.py::test_fail_{i}", "failed", "f" * 90)
    failures = [item["test"] for item in summary.result["tests"] if item["outcome"] == "failed"]
    assert failures and not summary.result["note"], "start with a complete head summary"
    result = call(submission, summary.result)
    assert len(json.dumps(result).encode()) <= SUMMARY_BYTES
    assert "Truncated" in result["note"]
    assert [item["test"] for item in result["tests"] if item["outcome"] == "failed"] == failures
    assert sum(item["outcome"] == "passed" for item in result["tests"]) < 60
    assert result["totals"] == summary.result["totals"]


def test_request_2709_legacy_plan_extends_before_final_head(
        generator_repo, generator_backend, tmp_path, monkeypatch):
    from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
    from langchain_core.messages import AIMessage
    from tl3_workers import runtime
    from tl3_workers.contracts import validate
    for name in ("socket_support.py", "test_short_socket.py"):
        (generator_repo / "tests" / name).unlink()
    (generator_repo / "tests/test_wait.py").write_text("from socket import socket\n")
    done = AIMessage(content=json.dumps({"kind": "done", "self_check": [
        {"goal_id": "import", "status": "met", "evidence": "Updated test import."}]}))
    turns = [AIMessage(content="", tool_calls=[{"name": "write", "id": "edit", "args": {
        "path": "tests/test_wait.py", "content": "from pathlib import Path\n"}}]), done]
    monkeypatch.setattr(runtime, "create_model", lambda _: FakeMessagesListChatModel(responses=turns))
    request = {"protocol": 1, "role": "maker", "worktree": str(generator_repo),
        "operation": {"node": 1, "generation": 0, "kind": "make"},
        "inputs": {"brief": "Update only the test's import; never hand-edit generated files.",
            "goals": [{"id": "import", "description": "Update the test import."}],
            "plan": {"files": ["tests/test_wait.py"], "checks": [{"name": "import", "kind": "demonstrating",
                "tests": ["tests/test_wait.py::test_wait"], "goal_ids": ["import"]}]},
            "forecast": {"fit_tokens": 100000, "context_tokens": 150000, "overhead_tokens": {"work": 1},
                "generated_files": ["**/BUILD.bazel"], "generator": "python3 generate.py", "generator_available": True},
            "attempt": 1, "round": 0, "context_size": 150000,
            "tests": {"image": IMAGE, "tools_path": str(tmp_path / "tools")}}}
    events = []
    runtime.run(request, events.append)
    extension = events[-1]["reply"]
    assert extension["kind"] == "needs_files", extension
    validate("maker-reply", extension)
    assert extension["fits"] is True
    assert [entry["path"] for entry in extension["files"]] == ["tests/BUILD.bazel"]
    # The existing controller extension transition locks these files before the next make.
    request["inputs"]["plan"]["files"].extend(entry["path"] for entry in extension["files"])
    monkeypatch.setattr(runtime, "create_model", lambda _: FakeMessagesListChatModel(responses=[done]))
    runtime.run(request, events.append)
    result = events[-1]["reply"]
    assert result["kind"] == "done", result
    validate("maker-reply", result)
    tree = subprocess.check_output(["git", "rev-parse", result["head"] + "^{tree}"], cwd=generator_repo).decode().strip()
    assert result["generation_evidence"]["output_tree"] == tree
    subprocess.run(["python3", "generate.py", "--check"], cwd=generator_repo, check=True)
