"""Shared maker fixtures and tool-call invariants."""
from langchain_core.messages import AIMessage, ToolMessage
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
import pytest
from tl3_workers.roles import ROLES
from tl3_workers.roles import maker
import worker_process


def assert_tool_pairs(messages):
    pending = set()
    for message in messages:
        if isinstance(message, AIMessage):
            assert not pending, f"missing tool results: {pending}"
            pending = {call["id"] for call in message.tool_calls}
            assert len(pending) == len(message.tool_calls), "duplicate tool call IDs"
        elif isinstance(message, ToolMessage):
            assert message.tool_call_id in pending, "tool result without preceding call"
            pending.remove(message.tool_call_id)
        else:
            assert not pending, "instruction interrupts pending tool results"
    assert not pending, f"missing tool results: {pending}"


DONE = {
    "kind": "done",
    "self_check": [
        {"goal_id": "greeting", "status": "met", "evidence": "Greeting now returns hello."},
        {"goal_id": "interface", "status": "met", "evidence": "The greet() signature is unchanged."},
    ],
}


NO_USAGE = [None, {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}]


class ToolModel(FakeMessagesListChatModel):
    def bind_tools(self, tools, **kwargs):
        assert {tool.name: tool.description for tool in tools} == ROLES["maker"].tools
        return self


@pytest.fixture(autouse=True)
def tool_capable_process_model(monkeypatch):
    launcher = worker_process.LAUNCH.replace(
        "model = FakeMessagesListChatModel(",
        """class MakerModel(FakeMessagesListChatModel):
    def bind_tools(self, tools, **kwargs):
        from tl3_workers.roles import ROLES
        assert {tool.name: tool.description for tool in tools} == dict(ROLES['maker'].tools)
        return self
model = MakerModel(""",
    )
    monkeypatch.setattr(worker_process, "LAUNCH", launcher)


def turn(reply=None, *, calls=None, input_tokens=100, output_tokens=50, reasoning=None):
    usage = {"input_tokens": input_tokens, "output_tokens": output_tokens,
             "total_tokens": input_tokens + output_tokens}
    if reasoning is not None:
        usage["output_token_details"] = {"reasoning": reasoning}
    return {
        "content": json.dumps(reply) if reply is not None else "",
        "tool_calls": calls or [],
        "usage_metadata": usage,
    }


def write_turn(path="greeting.py", content="def greet(): return 'hello'\n", **usage):
    return turn(calls=[{"name": "write", "args": {"path": path, "content": content},
                        "id": "write", "type": "tool_call"}], **usage)


def git(root, *args):
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env.update({
        "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_AUTHOR_NAME": "Fixture", "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
        "GIT_COMMITTER_NAME": "Fixture", "GIT_COMMITTER_EMAIL": "fixture@example.invalid",
    })
    return subprocess.run(
        ["git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false", *args],
        cwd=root, env=env, check=True, capture_output=True, text=True,
    ).stdout.strip()


@pytest.fixture(scope="session")
def cache_tokenizer(tmp_path_factory):
    from tiktoken.load import read_file_cached
    from tl3_workers.forecast import TOKENIZER_URL, TOKENIZER_SHA256

    data = read_file_cached(TOKENIZER_URL, TOKENIZER_SHA256)
    cache = tmp_path_factory.mktemp("maker-tokenizer")
    (cache / hashlib.sha1(TOKENIZER_URL.encode()).hexdigest()).write_bytes(data)
    return str(cache)


@pytest.fixture
def maker_request(tmp_path):
    root = tmp_path / "repo"
    root.mkdir()
    git(root, "init", "--quiet", "--template=")
    (root / "greeting.py").write_text("def greet(): return 'goodbye'\n")
    git(root, "add", ".")
    git(root, "commit", "--quiet", "-m", "Base")
    return {
        "protocol": 1,
        "operation": {"node": 1, "generation": 0, "kind": "make"},
        "role": "maker",
        "model": {"base_url": "http://model.invalid/v1", "name": "fixture-model",
                  "effort": "high", "budget_tokens": 32000},
        "key_file": str(tmp_path / "unused-key"),
        "worktree": str(root),
        "inputs": {
            "brief": "Return hello without changing greet's interface.",
            "goals": [{"id": "greeting", "description": "Return hello.", "verification": "test"},
                      {"id": "interface", "description": "Keep greet's interface.", "verification": "review"}],
            "plan": {
                "files": ["greeting.py", "test_greeting.py"],
                "checks": [
                    {"name": "hello", "kind": "demonstrating",
                     "tests": ["test_greeting.py::test_hello"], "goal_ids": ["greeting"]},
                    {"name": "signature", "kind": "behavior-preserving",
                     "tests": ["test_greeting.py::test_signature"], "goal_ids": ["interface"]},
                ],
            },
            "attempt": 1, "round": 0,
            "tests": {"image": "sha256:" + "a" * 64,
                      "tools_path": "/mnt/shared/prismabuild-fleet/repo/tools"},
            "context_size": 1000,
        },
    }


def reasoning_usage(input_tokens, output_tokens, reasoning):
    usage = {"input_tokens": input_tokens, "output_tokens": output_tokens,
             "total_tokens": input_tokens + output_tokens}
    if reasoning is not None:
        usage["output_token_details"] = {"reasoning": reasoning}
    return usage


def read_call(path="greeting.py"):
    return [{"name": "read", "args": {"path": path}, "id": "read", "type": "tool_call"}]


DONE_TOKENS = (len(json.dumps(DONE)) + 2) // 3


def requested_fit(maker_request, paths):
    from tl3_workers.tools import Worktree

    response = {'kind': 'needs_files', 'files': [{'path': path, 'why': 'Needed.'} for path in paths]}
    with Worktree(maker_request['worktree']) as tree:
        return maker.run(ToolModel(responses=[AIMessage(**turn(response))]),
                         maker_request, tree, lambda event: None)['fits']


@pytest.fixture
def review_fix(maker_request):
    inputs = maker_request['inputs']
    inputs.update(round=1, reviewed_sha='a' * 40, feedback=[
        {'kind': 'review_finding', 'id': 'review-4-0', 'reviewed_sha': 'a' * 40,
         'report': 'Greeting fails for an empty name.', 'goal_ids': ['greeting']},
        {'kind': 'review_finding', 'id': 'review-4-1', 'reviewed_sha': 'a' * 40,
         'report': 'Describe the signature.', 'goal_ids': ['interface']},
        {'kind': 'failed_check', 'report': 'Existing test failed.', 'goal_ids': ['greeting']},
        {'kind': 'review_finding', 'id': 'old-review', 'reviewed_sha': 'b' * 40,
         'report': 'An earlier attempt failed.', 'goal_ids': ['greeting']},
    ])
    inputs['current_review_findings'] = inputs['feedback'][:2]
    reply = {**DONE, 'reproductions': [
        {'finding_id': 'review-4-0', 'tests': ['test_greeting.py::test_empty_name']},
        {'finding_id': 'review-4-1', 'reason': 'This is a documentation correction.'},
    ]}
    return inputs, reply


def pending_conflict(request):
    root = Path(request['worktree'])
    base = git(root, 'rev-parse', 'HEAD')
    (root / 'greeting.py').write_text("def greet(): return 'leaf'\n")
    git(root, 'add', '.')
    git(root, 'commit', '-m', 'Leaf')
    leaf = git(root, 'rev-parse', 'HEAD')
    branch = git(root, 'symbolic-ref', '--short', 'HEAD')
    git(root, 'checkout', '-b', 'upstream', base)
    (root / 'greeting.py').write_text("def greet(): return 'upstream'\n")
    git(root, 'add', '.')
    git(root, 'commit', '-m', 'Main')
    target = git(root, 'rev-parse', 'HEAD')
    git(root, 'checkout', branch)
    with pytest.raises(subprocess.CalledProcessError):
        git(root, 'merge', '--no-commit', target)
    return root, leaf, target


RECEIPT = {"action_key": "a" * 64, "status": "executed", "exit_code": 0,
           "receipt_sha256": "b" * 64, "log_sha256": "c" * 64}


def staged_tree(root):
    # The tree of the worktree's current files, as pb_tests stages them for PrismaBuild.
    with tempfile.TemporaryDirectory() as temporary:
        env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        env.update(GIT_INDEX_FILE=str(Path(temporary) / "index"), GIT_CONFIG_NOSYSTEM="1",
                   GIT_CONFIG_GLOBAL=os.devnull)
        subprocess.run(["git", "add", "--all"], cwd=root, env=env, check=True, capture_output=True)
        return subprocess.run(["git", "write-tree"], cwd=root, env=env, check=True,
                              capture_output=True, text=True).stdout.strip()


def receipts_factory(worktree):
    def run_tests(tests: list[str] | None = None, base: bool = False, repeat: int = 1,
                  parallel: int = 0, priority: int | None = None) -> dict:
        return {"status": "completed", "totals": {"passed": 1, "failed": 0, "error": 0, "skipped": 0},
                "tests": [], "note": "", "action_key": "a" * 64, "receipts": [RECEIPT],
                "tree": staged_tree(worktree.test_checkout),
                "repeat": repeat, "runs": [{"run": n + 1, "status": "completed",
                                            "totals": {"passed": 1, "failed": 0, "error": 0, "skipped": 0},
                                            "failed": []} for n in range(repeat)]}
    return run_tests


RUN_FIVE = AIMessage(**turn(calls=[{"name": "run_tests", "id": "test", "type": "tool_call",
                                    "args": {"tests": ["test_greeting.py::test_hello"], "repeat": 5}}]))


@pytest.fixture
def request_123_handover(maker_request):
    """Sanitized fail-fast shape: test repair, Q55 proof, and mixed scope/PR goal."""
    request = maker_request
    request["inputs"]["goals"] = [
        {"id": "repair", "description": "Make the deep-path test deterministic.", "verification": "test"},
        {"id": "scope", "description": "Change only the declared ordinary test file.", "verification": "review"},
        {"id": "forced-proof", "description": "Independently prove forced base red and head green.",
         "verification": "test", "stage": "checking"},
        {"id": "publication", "description": "Publish a stable branch and non-draft PR with proof.",
         "verification": "review", "stage": "publishing",
         "publication": {"non_draft": True, "body_lines": ["Change type: test"], "proof": True}},
    ]
    request["inputs"]["plan"]["checks"] = [{
        "name": "deep-path", "kind": "demonstrating", "tests": ["test_greeting.py::test_hello"],
        "goal_ids": ["repair", "scope", "forced-proof"],
        "forcing": {"symptom": "The old test depends on residency.", "environment": {},
                    "patches": [{"target": "greeting.RESIDENCY", "value": False}]},
    }]
    reply = {"kind": "done", "self_check": [
        {"goal_id": "repair", "status": "met", "evidence": "Normal head run passed; plain base is unforced."},
        {"goal_id": "scope", "status": "met", "evidence": "Diff stays within the declared footprint."},
        {"goal_id": "forced-proof", "status": "pending", "why": "Controller Checking owns the Q55 verdict."},
        {"goal_id": "publication", "status": "pending", "why": "Publishing owns Git and PR creation."},
    ]}
    return request, reply
