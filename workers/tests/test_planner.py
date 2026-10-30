import copy
import json
import os

import pytest
import tiktoken

from tl3_workers.contracts import validate
import worker_process
from worker_process import start_worker


LEAF = {
    "kind": "leaf",
    "files": ["greeting.py", "tests/test_greeting.py"],
    "checks": [
        {"kind": "demonstrating", "node_ids": ["tests/test_greeting.py::test_hello"],
         "goal_ids": [0]},
        {"kind": "behavior-preserving", "node_ids": ["tests/test_greeting.py::test_api"],
         "goal_ids": [1]},
        {"kind": "regression", "node_ids": ["tests/test_greeting.py::test_existing"],
         "goal_ids": [0, 1]},
    ],
    "no_design_questions": True,
    "lands_on_its_own": True,
}
SPLIT = {
    "kind": "split", "reason": "does_not_fit",
    "pieces": [
        {"brief": "Fix the greeting.", "goal_ids": [0], "files": ["greeting.py"]},
        {"brief": "Keep the API.", "goal_ids": [1], "files": ["greeting.py", "new/api.py"]},
    ],
}
CANT_MEET = {"kind": "cant_meet", "report": "The required API decision is unresolved.",
             "files": []}


@pytest.fixture(scope="session", autouse=True)
def cache_tokenizer(tmp_path_factory):
    # Provision public tokenizer data before the per-test socket denial. Worker
    # subprocesses then use this cache while all model and tool calls stay offline.
    with pytest.MonkeyPatch.context() as patch:
        patch.setenv("TIKTOKEN_CACHE_DIR", str(tmp_path_factory.mktemp("tokenizer")))
        tiktoken.get_encoding("o200k_base")
        yield


@pytest.fixture(autouse=True)
def tool_capable_fake(monkeypatch):
    # The shared launcher uses the tool-less fake needed by ingest and audit.
    # Adapt only that fake; production tool binding and dispatch still run.
    setup = '''
from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
def bind_tools(self, tools, **kwargs):
    assert {tool.name for tool in tools} == {'list', 'read', 'glob', 'grep'}
    assert all(tool.description for tool in tools)
    return self
FakeMessagesListChatModel.bind_tools = bind_tools
'''
    monkeypatch.setattr(worker_process, "LAUNCH", setup + worker_process.LAUNCH)


@pytest.fixture
def planner_request(tmp_path):
    worktree = tmp_path / "worktree"
    worktree.mkdir()
    (worktree / "greeting.py").write_text("hello world\n")
    return {
        "protocol": 1,
        "operation": {"node": 1, "generation": 0, "kind": "planning"},
        "role": "planner",
        "model": {"base_url": "http://model.invalid/v1", "name": "fixture-model",
                  "effort": "high", "budget_tokens": 32000},
        "key_file": str(tmp_path / "never-read-key"),
        "inputs": {
            "must_split": False,
            "findings": [],
            "protected_paths": [".github/**", ".mergify.yml", "**/conftest.py",
                                "pytest.ini", "tox.ini", "noxfile.py", "setup.cfg",
                                "pyproject.toml"],
            "ingest": {
                "kind": "brief", "brief": "hello world",
                "goals": [{"description": "Return hello.", "verification": "test"},
                          {"description": "Keep the API clear.", "verification": "review"}],
                "memory_ids": [],
            },
            "forecast": {"fit_tokens": 100000, "context_tokens": 150000,
                         "overhead_tokens": {"prompt": 10, "making": 20, "review": 30}},
        },
        "worktree": str(worktree),
    }


def run_reply(tmp_path, request, reply, calls=()):
    turns = []
    if calls:
        turns.append({"content": "", "tool_calls": list(calls)})
    turns.append({"content": json.dumps(reply)})
    return start_worker(tmp_path, request, turns)


def assert_rejected(result, phase="run", message=None):
    assert result.exit_code == 1
    assert not any(event["event"] == "reply" for event in result.events)
    expected = f"tl3_workers: {phase} failed"
    if message:
        expected = f"tl3_workers: planner: {message}\n{expected}"
    assert result.stderr.strip() == expected


@pytest.mark.parametrize("cache", ["unset", "disabled", "empty", "corrupt"])
def test_tokenizer_must_be_preloaded_without_network(tmp_path, planner_request, monkeypatch, cache):
    if cache == "unset":
        monkeypatch.setenv("DATA_GYM_CACHE_DIR", os.environ["TIKTOKEN_CACHE_DIR"])
        monkeypatch.delenv("TIKTOKEN_CACHE_DIR", raising=False)
    elif cache == "disabled":
        monkeypatch.setenv("TIKTOKEN_CACHE_DIR", "")
    else:
        directory = tmp_path / "tokenizer"
        directory.mkdir()
        if cache == "corrupt":
            (directory / "fb374d419588a4632f3f557e76b4b70aebbca790").write_bytes(b"bad cache")
        monkeypatch.setenv("TIKTOKEN_CACHE_DIR", str(directory))
    assert_rejected(
        run_reply(tmp_path, planner_request, LEAF),
        message="o200k_base tokenizer data must be preloaded in TIKTOKEN_CACHE_DIR",
    )


@pytest.mark.parametrize("content,message", [
    (b"x" * (1_048_576 + 1), "declared file exceeds the 1048576-byte limit"),
    (b"hello\x00world", "declared file must be UTF-8 text without binary control bytes"),
    (b"\xff\xfe", "declared file must be UTF-8 text without binary control bytes"),
], ids=["oversized", "binary", "non_utf8"])
def test_invalid_file_has_clear_error(tmp_path, planner_request, content, message):
    (tmp_path / "worktree/greeting.py").write_bytes(content)
    assert_rejected(run_reply(tmp_path, planner_request, LEAF), message=message)


def test_out_of_room_rejects_leaf(tmp_path, planner_request, monkeypatch):
    from tl3_workers.roles.planner import check_reply

    planner_request["inputs"]["must_split"] = True
    monkeypatch.chdir(planner_request["worktree"])
    reply = copy.deepcopy(LEAF)
    check_reply(planner_request["inputs"], reply)
    assert "Must split after running out of room." in reply["violations"]


def test_out_of_room_leaf_is_rejected_through_protocol(tmp_path, planner_request):
    planner_request["inputs"]["must_split"] = True
    assert_rule_report(run_reply(tmp_path, planner_request, LEAF), "Must split")


@pytest.mark.parametrize("proposal", [SPLIT, CANT_MEET])
def test_out_of_room_can_split_or_report_obstacle(tmp_path, planner_request, proposal):
    planner_request["inputs"]["must_split"] = True
    planner_request["inputs"]["findings"] = ["The maker exhausted its context."]
    proposal = copy.deepcopy(proposal)
    if proposal["kind"] == "split":
        proposal["reason"] = "out_of_room"
    result = run_reply(tmp_path, planner_request, proposal)
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]["reply"]["kind"] == proposal["kind"]


def test_protected_path_is_rejected_through_protocol(tmp_path, planner_request):
    assert_rule_report(run_reply(tmp_path, planner_request, {**LEAF, "files": ["conftest.py"]}), "protected path")


def test_file_at_byte_limit_is_counted_in_full(tmp_path, planner_request):
    content = (b"hello world\n" * 90_000)[:1_048_576]
    (tmp_path / "worktree/greeting.py").write_bytes(content)
    result = run_reply(tmp_path, planner_request, LEAF)
    assert result.exit_code == 0, result.stderr
    expected = len(tiktoken.get_encoding("o200k_base").encode_ordinary(content.decode()))
    assert result.events[-1]["reply"]["forecast"]["file_tokens"]["greeting.py"] == expected


def test_local_tokenizer_matches_pinned_encoding_without_loader(monkeypatch):
    from tl3_workers.forecast import _encoding

    expected = tiktoken.get_encoding("o200k_base")

    def no_loader(*args, **kwargs):
        raise AssertionError("planner must not call the downloading tokenizer loader")

    monkeypatch.setattr(tiktoken, "get_encoding", no_loader)
    actual = _encoding()
    for content in ["Hello, WORLD! I'm testing.\r\n", "こんにちは 🌍 مرحبا नमस्ते",
                    "<|endoftext|> <|endofprompt|>", "123456789\t\n  punctuation!?!"]:
        assert actual.encode_ordinary(content) == expected.encode_ordinary(content)
    assert actual.encode("<|endoftext|>", allowed_special="all") == [199999]
    assert actual.encode("<|endofprompt|>", allowed_special="all") == [200018]


@pytest.mark.parametrize("field", ["must_split", "findings", "protected_paths"])
def test_controller_must_supply_planning_inputs(tmp_path, planner_request, field):
    del planner_request["inputs"][field]
    assert_rejected(run_reply(tmp_path, planner_request, LEAF), "request")


@pytest.mark.parametrize("proposal", [LEAF, SPLIT])
@pytest.mark.parametrize("path,pattern", [
    (".github/workflows/test.yml", ".github/**"),
    (".mergify.yml", ".mergify.yml"),
    ("conftest.py", "**/conftest.py"),
    ("tests/nested/conftest.py", "**/conftest.py"),
    ("pytest.ini", "pytest.ini"),
    ("tox.ini", "tox.ini"),
    ("noxfile.py", "noxfile.py"),
    ("setup.cfg", "setup.cfg"),
    ("pyproject.toml", "pyproject.toml"),
    ("src/private.py", "src/*.py"),
])
def test_protected_paths_are_rejected(tmp_path, planner_request, monkeypatch, proposal, path, pattern):
    from tl3_workers.roles.planner import check_reply

    planner_request["inputs"]["protected_paths"] = [pattern]
    reply = copy.deepcopy(proposal)
    if reply["kind"] == "leaf":
        reply["files"] = [path]
    else:
        reply["pieces"][1]["files"] = [path]
    monkeypatch.chdir(planner_request["worktree"])
    with pytest.raises(ValueError, match="declared file matches a protected path"):
        check_reply(planner_request["inputs"], reply)


@pytest.mark.parametrize("proposal,files", [
    (LEAF, {"greeting.py": 3, "tests/test_greeting.py": 0}),
    (SPLIT, {"greeting.py": 3, "new/api.py": 0}),
    (CANT_MEET, {}),
    ({**CANT_MEET, "files": ["greeting.py"]}, {"greeting.py": 3}),
])
def test_each_reply_has_code_forecast(tmp_path, planner_request, proposal, files):
    result = run_reply(tmp_path, planner_request, proposal)
    assert result.exit_code == 0, result.stderr
    reply = result.events[-1]["reply"]
    assert result.events == [
        {"event": "progress", "unit": "model_turn"},
        {"event": "reply", "role": "planner", "reply": reply},
    ]
    assert {key: value for key, value in reply.items() if key != "violations"} == {**proposal, "forecast": {
        "tokenizer": "o200k_base", "brief_tokens": 2, "file_tokens": files,
        "overhead_tokens": 60, "tokens": 62 + sum(files.values()),
        "fit_tokens": 100000, "context_tokens": 150000, "fits": True,
    }}
    validate("planner-reply", reply)


@pytest.mark.parametrize("limit,fits", [(66, True), (65, False), (64, False)])
def test_fit_boundary_uses_config(tmp_path, planner_request, limit, fits):
    planner_request["inputs"]["forecast"]["fit_tokens"] = limit
    result = run_reply(tmp_path, planner_request, LEAF)
    assert result.exit_code == 0, result.stderr
    forecast = result.events[-1]["reply"]["forecast"]
    assert forecast["tokens"] == 65
    assert forecast["fits"] is fits
    assert forecast["fit_tokens"] == limit


def test_forecast_reads_full_unicode_file_without_tool_truncation(tmp_path, planner_request):
    content = ("こんにちは 🌍 <|endoftext|>\r\n" * 700) + "last line"
    path = tmp_path / "worktree/greeting.py"
    path.write_bytes(content.encode())
    planner_request["inputs"]["ingest"]["brief"] = "<|endoftext|>"
    result = run_reply(tmp_path, planner_request, LEAF)
    assert result.exit_code == 0, result.stderr
    forecast = result.events[-1]["reply"]["forecast"]
    encoding = tiktoken.get_encoding("o200k_base")
    assert forecast["file_tokens"]["greeting.py"] == len(encoding.encode_ordinary(content))
    assert forecast["brief_tokens"] == len(encoding.encode_ordinary("<|endoftext|>"))


@pytest.mark.parametrize("change", [
    {"checks": []},
    {"checks": [LEAF["checks"][0]]},
    {"checks": [{**LEAF["checks"][0], "goal_ids": [0, 1, 2]}]},
    {"checks": [{**LEAF["checks"][0], "goal_ids": []}]},
    {"checks": [{**LEAF["checks"][0], "node_ids": []}]},
    {"checks": [{**LEAF["checks"][0], "kind": "guess"}]},
    {"checks": [{**LEAF["checks"][0], "node_ids": ["tests/test_greeting.py"]}]},
    {"checks": [{**LEAF["checks"][0], "node_ids": ["../test.py::test_escape"]}]},
    {"files": []},
    {"files": ["greeting.py", "greeting.py"]},
    {"no_design_questions": False},
    {"lands_on_its_own": False},
])
def test_invalid_leaf_never_leaves_worker(tmp_path, planner_request, change):
    result = run_reply(tmp_path, planner_request, {**LEAF, **change})
    if result.exit_code == 0:
        assert_rule_report(result)
    else:
        assert_rejected(result)


@pytest.mark.parametrize("path", [
    "../outside.py", "/etc/passwd", "new/../../outside.py", "new/../outside.py",
    "new\\outside.py", ".git/config", "new/.git/config", "./greeting.py",
    "new//file.py", "greeting.py/", ".", "bad\x00path",
])
def test_declared_paths_are_confined(tmp_path, planner_request, path):
    assert_rejected(run_reply(tmp_path, planner_request, {**LEAF, "files": [path]}))


@pytest.mark.parametrize("link_kind", ["file", "directory", "dangling", "hard", "fifo", "directory_file"])
def test_forecaster_refuses_unsafe_files(tmp_path, planner_request, link_kind):
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "secret.py").write_text("fixture only")
    target = tmp_path / "worktree/unsafe"
    path = "unsafe"
    if link_kind == "file":
        target.symlink_to(outside / "secret.py")
    elif link_kind == "directory":
        target.symlink_to(outside, target_is_directory=True)
        path += "/new.py"
    elif link_kind == "dangling":
        target.symlink_to(outside / "missing.py")
    elif link_kind == "hard":
        os.link(outside / "secret.py", target)
    elif link_kind == "fifo":
        os.mkfifo(target)
    else:
        target.mkdir()
    assert_rejected(run_reply(tmp_path, planner_request, {**LEAF, "files": [path]}))


@pytest.mark.parametrize("proposal", [
    {**SPLIT, "pieces": SPLIT["pieces"][:1]},
    {**SPLIT, "reason": "convenience"},
    {**SPLIT, "pieces": [SPLIT["pieces"][0], SPLIT["pieces"][0]]},
    {**SPLIT, "pieces": [SPLIT["pieces"][0], {**SPLIT["pieces"][1], "goal_ids": [99]}]},
    {**SPLIT, "pieces": [SPLIT["pieces"][0], {**SPLIT["pieces"][1], "files": ["../escape"]}]},
    {**CANT_MEET, "report": ""},
    {**CANT_MEET, "files": ["../escape"]},
])
def test_invalid_alternatives_never_leave_worker(tmp_path, planner_request, proposal):
    result = run_reply(tmp_path, planner_request, proposal)
    if result.exit_code == 0:
        assert_rule_report(result)
    else:
        assert_rejected(result)


def test_model_cannot_supply_its_own_forecast(tmp_path, planner_request):
    good = run_reply(tmp_path, planner_request, LEAF)
    assert good.exit_code == 0, good.stderr
    forged = good.events[-1]["reply"]
    assert_rejected(run_reply(tmp_path, planner_request, forged))


@pytest.mark.parametrize("change", [
    {"fit_tokens": 150000}, {"context_tokens": 0}, {"fit_tokens": 0},
    {"overhead_tokens": {}}, {"overhead_tokens": {"making": -1}},
])
def test_invalid_config_is_rejected(tmp_path, planner_request, change):
    planner_request["inputs"]["forecast"].update(change)
    phase = "run" if change == {"fit_tokens": 150000} else "request"
    assert_rejected(run_reply(tmp_path, planner_request, LEAF), phase)


def test_early_ingest_end_is_not_plannable(tmp_path, planner_request):
    planner_request["inputs"]["ingest"] = {
        "kind": "early_end", "outcome": "duplicate", "reason": "Another request covers it.",
    }
    assert_rejected(run_reply(tmp_path, planner_request, LEAF), "request")


def test_read_only_tools_through_protocol(tmp_path, planner_request):
    calls = [
        {"name": name, "args": args, "id": name, "type": "tool_call"}
        for name, args in [
            ("list", {}), ("read", {"path": "greeting.py"}),
            ("glob", {"pattern": "*.py"}), ("grep", {"pattern": "hello"}),
            ("write", {"path": "forbidden.py", "content": "no"}),
            ("edit", {"path": "greeting.py", "old": "hello", "new": "changed"}),
        ]
    ]
    result = run_reply(tmp_path, planner_request, copy.deepcopy(LEAF), calls)
    assert result.exit_code == 0, result.stderr
    assert result.events[:-1] == [
        {"event": "progress", "unit": "model_turn"},
        *[{"event": "progress", "unit": "tool_result"} for _ in calls],
        {"event": "progress", "unit": "model_turn"},
    ]
    assert not (tmp_path / "worktree/forbidden.py").exists()
    assert (tmp_path / "worktree/greeting.py").read_text() == "hello world\n"
    assert result.events[-1]["reply"]["forecast"]["file_tokens"]["greeting.py"] == 3


def assert_rule_report(result, reason=""):
    assert result.exit_code == 0, result.stderr
    violations = result.events[-1]["reply"]["violations"]
    assert violations and any(reason in report for report in violations)


def test_model_cannot_forge_rule_evidence(tmp_path, planner_request):
    assert_rejected(run_reply(tmp_path, planner_request, {**LEAF, "violations": []}))


def test_split_requires_evidence_and_smaller_children(tmp_path, planner_request):
    reply = run_reply(tmp_path, planner_request, SPLIT)
    assert_rule_report(reply, "forecast fits")
    assert_rule_report(reply, "smaller forecast")


def test_fit_failure_is_reported_to_kernel(tmp_path, planner_request):
    planner_request["inputs"]["forecast"]["fit_tokens"] = 65
    assert_rule_report(run_reply(tmp_path, planner_request, LEAF), "does not fit")
