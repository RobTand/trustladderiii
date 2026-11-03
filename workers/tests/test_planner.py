import copy
import json
import os
from pathlib import Path

import pytest
from jsonschema import ValidationError
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
        {"kind": "behavior-preserving", "node_ids": ["tests/test_existing.py::test_api"],
         "goal_ids": [1]},
        {"kind": "regression", "node_ids": ["tests/test_existing.py::test_existing"],
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
        from tiktoken.load import read_file_cached
        from tl3_workers.forecast import TOKENIZER_URL, TOKENIZER_SHA256
        read_file_cached(TOKENIZER_URL, TOKENIZER_SHA256)
        yield


@pytest.fixture(autouse=True)
def tool_capable_fake(monkeypatch):
    # Check the shared fake binds precisely the planner's read-only tools.
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
    (worktree / "tests").mkdir()
    (worktree / "tests/test_existing.py").write_text(
        "def test_api(): assert True\ndef test_existing(): assert True\n")
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
    (b"hello\x00world", "greeting.py: binary (11 bytes)"),
    (b"\xff\xfe", "greeting.py: not UTF-8 text (2 bytes)"),
], ids=["binary", "non_utf8"])
def test_invalid_file_has_clear_error(tmp_path, planner_request, content, message):
    # #321: a deterministic file problem is a named violation, not a crash a retry repeats.
    (tmp_path / "worktree/greeting.py").write_bytes(content)
    assert_rule_report(run_reply(tmp_path, planner_request, LEAF), message)


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


@pytest.mark.parametrize("extra", [0, 1])
def test_file_at_large_file_limit_is_counted_in_full(tmp_path, planner_request, extra):
    from tl3_workers.forecast import LARGE_FILE_BYTES, RANGED_FILE_TOKENS

    content = (b"hello world\n" * 6_000)[:LARGE_FILE_BYTES + extra]
    (tmp_path / "worktree/greeting.py").write_bytes(content)
    result = run_reply(tmp_path, planner_request, LEAF)
    assert result.exit_code == 0, result.stderr
    expected = len(tiktoken.get_encoding("o200k_base").encode_ordinary(content.decode()))
    # One byte over the limit, the file is read by range and costs a fixed allowance (#321).
    assert result.events[-1]["reply"]["forecast"]["file_tokens"]["greeting.py"] == (
        RANGED_FILE_TOKENS if extra else expected)


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
    check_reply(planner_request["inputs"], reply)
    assert any("protected path" in report for report in reply["violations"])


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


@pytest.mark.parametrize("change,reported", [
    ({"checks": []}, True),
    ({"checks": [LEAF["checks"][0]]}, True),
    ({"checks": [{**LEAF["checks"][0], "goal_ids": [0, 1, 2]}]}, True),
    ({"checks": [{**LEAF["checks"][0], "goal_ids": []}]}, True),
    ({"checks": [{**LEAF["checks"][0], "node_ids": []}]}, True),
    ({"checks": [{**LEAF["checks"][0], "kind": "guess"}]}, False),
    ({"checks": [{**LEAF["checks"][0], "node_ids": ["tests/test_greeting.py"]}]}, True),
    ({"files": []}, True),
    ({"files": ["greeting.py", "greeting.py"]}, False),
    ({"no_design_questions": False}, True),
    ({"lands_on_its_own": False}, True),
])
def test_leaf_rules_are_reported_and_malformed_shapes_fail(tmp_path, planner_request, change, reported):
    result = run_reply(tmp_path, planner_request, {**LEAF, **change})
    if reported:
        assert_rule_report(result)
    else:
        assert_rejected(result)


def test_invalid_node_id_is_reported_to_controller(tmp_path, planner_request):
    proposal = copy.deepcopy(LEAF)
    proposal["checks"][0]["node_ids"] = ["../test.py::test_escape"]
    result = run_reply(tmp_path, planner_request, proposal)
    assert_rule_report(result)
    assert result.events[-1]["reply"]["violations"] == ["Checks must name exact pytest node IDs."]


@pytest.mark.parametrize("path", [
    "../outside.py", "/etc/passwd", "new/../../outside.py", "new/../outside.py",
    "new\\outside.py", ".git/config", "new/.git/config", "./greeting.py",
    "new//file.py", "greeting.py/", ".", "bad\x00path",
])
def test_declared_paths_are_confined(tmp_path, planner_request, path):
    result = run_reply(tmp_path, planner_request, {**LEAF, "files": [path]})
    # #321: a named violation, not a crash. Nothing outside the worktree is measured.
    assert_rule_report(result, f"Declared file can't be used: {path}: ")
    assert result.events[-1]["reply"]["forecast"]["file_tokens"][path] == 0


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
    result = run_reply(tmp_path, planner_request, {**LEAF, "files": [path]})
    # #321: a named violation, not a crash. The outside file is never read.
    assert_rule_report(result, f"{path}: not a regular file inside the worktree")
    assert result.events[-1]["reply"]["forecast"]["file_tokens"][path] == 0


@pytest.mark.parametrize("proposal,reported", [
    ({**SPLIT, "pieces": SPLIT["pieces"][:1]}, True),
    ({**SPLIT, "reason": "convenience"}, False),
    ({**SPLIT, "pieces": [SPLIT["pieces"][0], SPLIT["pieces"][0]]}, True),
    ({**SPLIT, "pieces": [SPLIT["pieces"][0], {**SPLIT["pieces"][1], "goal_ids": [99]}]}, True),
    ({**SPLIT, "pieces": [SPLIT["pieces"][0], {**SPLIT["pieces"][1], "files": ["../escape"]}]}, True),
    ({**CANT_MEET, "report": ""}, False),
    ({**CANT_MEET, "files": ["../escape"]}, True),
])
def test_split_rules_are_reported_and_malformed_shapes_fail(tmp_path, planner_request, proposal, reported):
    result = run_reply(tmp_path, planner_request, proposal)
    if reported:
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


@pytest.mark.parametrize("proposal", [LEAF, SPLIT, CANT_MEET])
def test_one_verified_tokenizer_per_check(monkeypatch, planner_request, proposal):
    from tl3_workers import forecast
    from tl3_workers.roles.planner import check_reply

    builds = []
    original = forecast._encoding

    def counted_encoding():
        encoding = original()
        builds.append(encoding)
        return encoding

    monkeypatch.setattr(forecast, "_encoding", counted_encoding)
    monkeypatch.chdir(planner_request["worktree"])
    reply = copy.deepcopy(proposal)
    check_reply(planner_request["inputs"], reply)
    assert len(builds) == 1
    expected_files = {"greeting.py": 3, "new/api.py": 0} if proposal["kind"] == "split" else (
        {"greeting.py": 3, "tests/test_greeting.py": 0} if proposal["kind"] == "leaf" else {})
    assert reply["forecast"]["file_tokens"] == expected_files

    # A later check must revalidate cache bytes, even in the same process.
    monkeypatch.setenv("TIKTOKEN_CACHE_DIR", "")
    with pytest.raises(ValueError, match="must be preloaded"):
        check_reply(planner_request["inputs"], copy.deepcopy(proposal))


@pytest.mark.parametrize("vector", json.loads((Path(__file__).resolve().parents[2] / "harness/test-vectors/pytest-node-ids.json").read_text()), ids=lambda v: repr(v["node_id"]))
def test_planner_exact_node_ids_agree_with_harness(vector, planner_request, monkeypatch):
    from tl3_workers.roles import planner
    from tl3_workers.forecast import Forecaster
    monkeypatch.setattr(Forecaster, "measure", lambda *a: {"fits": True})
    reply = copy.deepcopy(LEAF)
    for check in reply["checks"]:
        check["node_ids"] = [vector["node_id"]]
    try:
        planner.check_reply(planner_request["inputs"], reply)
        valid = "Checks must name exact pytest node IDs." not in reply["violations"]
    except ValueError:
        valid = False
    assert valid == vector["valid"]


@pytest.mark.parametrize('proposal', [LEAF, SPLIT, CANT_MEET])
def test_planner_schema_follower_succeeds_without_code_only_fields(
        tmp_path, planner_request, monkeypatch, proposal):
    from test_sizing import FORGED
    from worker_process import fill_visible_fields

    fill_visible_fields(monkeypatch, {'forecast': FORGED['forecast'], 'violations': []})
    result = run_reply(tmp_path, planner_request, proposal)
    assert result.exit_code == 0, result.stderr
    actual = result.events[-1]['reply']
    assert actual['forecast']['fit_tokens'] == planner_request['inputs']['forecast']['fit_tokens']
    assert {key: value for key, value in actual.items()
            if key not in {'forecast', 'violations'}} == proposal
    assert bool(actual['violations']) == (proposal['kind'] == 'split')


@pytest.mark.parametrize('kind', ['regression', 'behavior-preserving'])
@pytest.mark.parametrize('missing', ['tests/absent.py::test_absent',
                                    'tests/test_sequences.py::test_absent',
                                    'tests/test_sequences.py::TestAbsent::test_absent'])
def test_planner_checks_base_test_definitions(planner_request, monkeypatch, kind, missing):
    from tl3_workers.roles.planner import check_reply
    from tl3_workers.forecast import Forecaster

    root = Path(planner_request['worktree'])
    (root / 'tests').mkdir(exist_ok=True)
    source = (Path(__file__).parents[2] / 'harness/tests/fixtures/chunked_test.py').read_text()
    (root / 'tests/test_sequences.py').write_text(source)
    monkeypatch.chdir(root)
    monkeypatch.setattr(Forecaster, 'measure', lambda *a: {'fits': True})
    reply = copy.deepcopy(LEAF)
    reply['checks'] = [{'kind': kind, 'node_ids': [missing], 'goal_ids': [0, 1]}]
    check_reply(planner_request['inputs'], reply)
    assert any(missing in item and 'base' in item for item in reply['violations'])
    reply = copy.deepcopy(LEAF)
    reply['checks'] = [{'kind': kind,
        'node_ids': ['tests/test_sequences.py::test_chunked_rejects_non_positive_sizes'],
        'goal_ids': [0, 1]}]
    check_reply(planner_request['inputs'], reply)
    assert reply['violations'] == []
    reply = copy.deepcopy(LEAF)
    reply['checks'] = [{'kind': 'demonstrating', 'node_ids': [missing], 'goal_ids': [0, 1]}]
    check_reply(planner_request['inputs'], reply)
    assert reply['violations'] == []


def test_new_unique_test_is_a_planner_defect(tmp_path, planner_request):
    root = Path(planner_request['worktree'])
    (root / 'tests').mkdir(exist_ok=True)
    (root / 'tests/test_sequences.py').write_text('def test_unique_preserves_order(): assert True\n')
    test = 'tests/test_sequences.py::test_unique_does_not_mutate_input'
    proposal = copy.deepcopy(LEAF)
    proposal['checks'] = [{'kind': 'regression', 'node_ids': [test], 'goal_ids': [0, 1]}]
    result = run_reply(tmp_path, planner_request, proposal)
    assert_rule_report(result, test)
    assert 'base' in result.events[-1]['reply']['violations'][0]


def test_prompt_states_how_existing_tests_are_named():
    # The planner gets one retry; a rule it was never told about spends it.
    prompt = " ".join((Path(__file__).parents[1] / "src/tl3_workers/prompts/planner.md").read_text().split())
    assert "must name tests that already exist in the base" in prompt
    assert "without its parameter suffix" in prompt
    assert "A test file path on its own is not a node ID" in prompt


def test_prompt_requires_deterministic_reproduction_for_determinism_work():
    # #340: a flaky test can pass on the base, so it cannot demonstrate a determinism fix.
    prompt = " ".join((Path(__file__).parents[1] / "src/tl3_workers/prompts/planner.md").read_text().split())
    assert "must name a test that reproduces the race deterministically" in prompt
    assert "a flaky test can pass on the base" in prompt


def test_prompt_states_forced_reproduction_for_deflakes():
    # #393, Rob 2026-11-02: a forced reproduction is the deflake red; stress runs don't count.
    prompt = " ".join((Path(__file__).parents[1] / "src/tl3_workers/prompts/planner.md").read_text().split())
    assert "forcing" in prompt and "forced reproduction" in prompt
    assert "fails by assertion on the base under the forcing" in prompt
    assert "passes on the change both with and without the forcing" in prompt
    assert "Repeating a test until it fails does not count" in prompt
    assert "A change to tests alone needs a forced reproduction (Q55, #393)" in prompt


PID_FORCING = {
    "symptom": "A real, unrelated process holds PID 4242, so the sampler cannot decide whether it left.",
    "environment": {},
    "patches": [{"target": "sampler.process_in_scope", "raises": "ValueError"}],
}


@pytest.fixture
def deflake_worktree(planner_request, monkeypatch):
    from tl3_workers.forecast import Forecaster

    root = Path(planner_request['worktree'])
    (root / 'tests').mkdir(exist_ok=True)
    (root / 'tests/test_sampler.py').write_text(
        'import sampler\n\n\ndef test_departed_process_is_not_live():\n'
        '    assert sampler.live_after([4242], [], "/fake") == 0\n')
    monkeypatch.chdir(root)
    monkeypatch.setattr(Forecaster, 'measure', lambda *a: {'fits': True})
    reply = copy.deepcopy(LEAF)
    reply['files'] = ['tests/test_sampler.py']
    reply['checks'] = [{'kind': 'demonstrating', 'goal_ids': [0, 1], 'forcing': copy.deepcopy(PID_FORCING),
                        'node_ids': ['tests/test_sampler.py::test_departed_process_is_not_live']}]
    return reply


def test_forced_reproduction_is_a_valid_plan(planner_request, deflake_worktree):
    from tl3_workers.roles.planner import check_reply

    validate("planner-reply", deflake_worktree)
    check_reply(planner_request['inputs'], deflake_worktree)
    assert deflake_worktree['violations'] == []


def test_forcing_belongs_only_on_demonstrating_checks(planner_request, deflake_worktree):
    from tl3_workers.roles.planner import check_reply

    deflake_worktree['checks'][0]['kind'] = 'regression'
    check_reply(planner_request['inputs'], deflake_worktree)
    assert "Only a demonstrating check can carry a forcing." in deflake_worktree['violations']


def test_forced_check_names_the_old_test_at_the_base(planner_request, deflake_worktree):
    from tl3_workers.roles.planner import check_reply

    test = 'tests/test_sampler.py::test_departure_is_stated'
    deflake_worktree['checks'][0]['node_ids'] = [test]
    check_reply(planner_request['inputs'], deflake_worktree)
    assert any(test in item and 'base' in item for item in deflake_worktree['violations'])


def test_invalid_forcing_is_a_planner_violation(planner_request, deflake_worktree):
    from tl3_workers.roles.planner import check_reply

    deflake_worktree['checks'][0]['forcing']['patches'][0]['raises'] = 'AssertionError'
    validate("planner-reply", deflake_worktree)
    check_reply(planner_request['inputs'], deflake_worktree)
    assert any(item.startswith('Invalid forcing:') and 'AssertionError' in item
               for item in deflake_worktree['violations'])


@pytest.mark.parametrize('change', [
    {'code': 'print(1)'},
    {'symptom': ''},
    {'patches': [{'target': 'sampler.f', 'exec': 'x'}]},
])
def test_planner_schema_keeps_the_forcing_declarative(deflake_worktree, change):
    import jsonschema

    deflake_worktree['checks'][0]['forcing'].update(change)
    with pytest.raises(jsonschema.ValidationError):
        validate("planner-reply", deflake_worktree)


def test_oversized_check_plan_is_a_planner_violation(planner_request, monkeypatch):
    from tl3_workers.roles.planner import check_reply
    monkeypatch.chdir(planner_request['worktree'])
    reply = copy.deepcopy(LEAF)
    reply['checks'] = [{'kind': 'demonstrating', 'goal_ids': [0, 1],
                        'node_ids': ['tests/test_greeting.py::test_' + 'x' * (4 * 1024 * 1024)]}]
    check_reply(planner_request['inputs'], reply)
    assert any('4194304' in violation and 'reduce' in violation for violation in reply['violations'])


@pytest.mark.parametrize('kind', ['regression', 'behavior-preserving'])
def test_existing_test_in_a_large_file_is_defined_at_the_base(planner_request, monkeypatch, kind):
    # A test module over 1 MiB still defines its tests; only the 16 MiB cap applies.
    from tl3_workers.roles.planner import check_reply
    from tl3_workers.forecast import Forecaster

    root = Path(planner_request['worktree'])
    (root / 'tests').mkdir(exist_ok=True)
    padding = "".join(f"def helper_{i}():\n    return {i}\n\n\n" for i in range(45_000))
    (root / 'tests/test_large.py').write_text(padding + "def test_existing():\n    assert True\n")
    assert (root / 'tests/test_large.py').stat().st_size > 1_048_576
    monkeypatch.chdir(root)
    monkeypatch.setattr(Forecaster, 'measure', lambda *a: {'fits': True})
    reply = copy.deepcopy(LEAF)
    reply['checks'] = [{'kind': kind, 'node_ids': ['tests/test_large.py::test_existing'], 'goal_ids': [0, 1]}]
    check_reply(planner_request['inputs'], reply)
    assert not any('test_large.py' in item for item in reply['violations']), reply['violations']


# #422: memories recalled upfront for planning.
MEMORIES = [{"id": "lesson-1", "kind": "lesson", "repo": "RobTand/tl3-sandbox",
             "content": "greet() returns 'hi'."},
            {"id": "run-1", "kind": "run", "content": "An earlier run read greeting.py."}]
FLAG = {"id": "lesson-1", "claim": "greet() returns 'hi'", "reason": "greeting.py says hello world.",
        "evidence": {"path": "greeting.py", "start_line": 1, "end_line": 1, "quote": "hello world"}}


def test_planner_accepts_memories_cites_flags_and_conventions(tmp_path, planner_request):
    planner_request["inputs"]["memories"] = MEMORIES
    reply = {**CANT_MEET, "memory_ids": ["lesson-1"], "memory_flags": [FLAG],
             "conventions": [{"text": "Greetings are plain text.", "evidence": FLAG["evidence"]}]}
    result = run_reply(tmp_path, planner_request, reply)
    assert result.exit_code == 0, result.stderr
    final = next(event for event in result.events if event["event"] == "reply")["reply"]
    assert final["memory_ids"] == ["lesson-1"]
    assert final["memory_flags"] == [FLAG]


def test_planner_inputs_may_note_a_recall_outage(tmp_path, planner_request):
    planner_request["inputs"]["memories"] = []
    planner_request["inputs"]["memory_note"] = "Hindsight recall was unavailable."
    result = run_reply(tmp_path, planner_request, CANT_MEET)
    assert result.exit_code == 0, result.stderr


@pytest.mark.parametrize("reply,message", [
    ({**CANT_MEET, "memory_ids": ["unknown"]}, "reply cites a memory it was not given"),
    ({**CANT_MEET, "memory_ids": [], "memory_flags": [FLAG]}, "reply flags a memory it did not cite"),
    ({**CANT_MEET, "memory_ids": ["lesson-1"], "memory_flags": [{**FLAG, "claim": "greet() raises errors"}]},
     "a flag's claim is not in the memory it flags"),
])
def test_planner_memory_citations_are_checked(planner_request, monkeypatch, reply, message):
    from tl3_workers.roles.planner import check_reply
    planner_request["inputs"]["memories"] = MEMORIES
    monkeypatch.chdir(planner_request["worktree"])
    with pytest.raises(ValueError, match=message):
        check_reply(planner_request["inputs"], copy.deepcopy(reply))


@pytest.mark.parametrize("flags", [[FLAG] * 4, [{key: FLAG[key] for key in FLAG if key != "claim"}]])
def test_planner_flags_are_bounded_by_schema(flags):
    reply = {**CANT_MEET, "memory_ids": ["lesson-1"], "memory_flags": flags}
    with pytest.raises(ValidationError):
        validate("planner-reply", reply)


def test_cant_meet_carries_required_protected_paths():
    reply = {**CANT_MEET, "protected_paths": ["tools/ci/bazel_gate.py"]}
    validate("planner-reply", reply)
    for invalid in [[], [1], [""], ["tools/ci/bazel_gate.py"] * 2]:
        with pytest.raises(ValidationError):
            validate("planner-reply", {**reply, "protected_paths": invalid})


DIGESTS = 'tests/test_digests.py'
FROZEN = f'{DIGESTS}::test_digests_imports_only_the_standard_library'
SIBLING = f'{DIGESTS}::test_digest_is_stable'


@pytest.fixture
def digests_plan(planner_request, monkeypatch):
    # #438: PQ #2662's shape. A goal extends an existing test that a check freezes.
    from tl3_workers.forecast import Forecaster

    root = Path(planner_request['worktree'])
    (root / 'digests.py').write_text('import hashlib\n')
    (root / DIGESTS).write_text(
        'import pytest\n\n'
        'def test_digests_imports_only_the_standard_library():\n    assert True\n\n'
        '@pytest.mark.parametrize("n", [1, 2])\n'
        'def test_digest_is_stable(n):\n    assert n\n')
    planner_request['inputs']['ingest']['goals'] = [
        {'description': 'Extend `test_digests_imports_only_the_standard_library` so the '
                        'standalone-loaded module force-executes its lazy imports.',
         'verification': 'test'},
        {'description': 'Keep digests stable.', 'verification': 'test'}]
    (root / 'pyproject.toml').write_text(
        '[tool.tl3.harness]\ncommand = ["python", "-m", "pytest"]\ntest-files = ["tests/**/*.py"]\n')
    monkeypatch.chdir(root)
    monkeypatch.setattr(Forecaster, 'measure', lambda *a: {'fits': True})
    return {
        'kind': 'leaf', 'files': ['digests.py', DIGESTS],
        'checks': [
            {'kind': 'demonstrating', 'node_ids': [f'{DIGESTS}::test_lazy_imports_run'], 'goal_ids': [0]},
            {'kind': 'regression', 'node_ids': [SIBLING], 'goal_ids': [1]},
            {'kind': 'regression', 'node_ids': [FROZEN], 'goal_ids': [0]},
        ],
        'no_design_questions': True, 'lands_on_its_own': True,
    }


def planned(planner_request, reply):
    from tl3_workers.roles.planner import check_reply

    check_reply(planner_request['inputs'], reply)
    return reply['violations']


def test_goal_that_edits_a_frozen_test_is_a_plan_defect(planner_request, digests_plan):
    violations = planned(planner_request, digests_plan)
    assert any('Goal 0 names `test_digests_imports_only_the_standard_library`' in item
               and 'check-3' in item and 'edits_tests' in item for item in violations), violations


def test_goal_naming_a_frozen_test_in_an_undeclared_file_is_not_flagged(planner_request, digests_plan):
    # The maker can't edit an undeclared file, so freezing its tests is right.
    digests_plan['files'] = ['digests.py']
    digests_plan['checks'][0]['node_ids'] = ['tests/test_lazy.py::test_lazy_imports_run']
    digests_plan['files'].append('tests/test_lazy.py')
    assert planned(planner_request, digests_plan) == []


@pytest.mark.parametrize('frozen', [FROZEN, f'{SIBLING}[1]'])
@pytest.mark.parametrize('kind', ['regression', 'behavior-preserving'])
def test_declared_edit_may_not_be_frozen(planner_request, digests_plan, frozen, kind):
    edited = frozen.split('[')[0]
    digests_plan['edits_tests'] = [edited]
    digests_plan['checks'] = [
        {'kind': 'demonstrating', 'node_ids': [edited], 'goal_ids': [0, 1]},
        {'kind': kind, 'node_ids': [frozen], 'goal_ids': [0, 1]},
    ]
    violations = planned(planner_request, digests_plan)
    assert any(f'`{edited}` is in edits_tests' in item and 'only in a demonstrating check' in item
               for item in violations), violations


def test_declared_edit_needs_a_demonstrating_check(planner_request, digests_plan):
    digests_plan['edits_tests'] = [FROZEN]
    digests_plan['checks'] = digests_plan['checks'][:2]
    digests_plan['checks'][0]['goal_ids'] = [0]
    violations = planned(planner_request, digests_plan)
    assert any(f'`{FROZEN}` is in edits_tests but no demonstrating check names it' in item
               for item in violations), violations


@pytest.mark.parametrize('edited,files,reason', [
    ('tests/test_digests.py', ['digests.py', DIGESTS], 'edits_tests must name exact pytest node IDs.'),
    (f'{DIGESTS}::test_new', ['digests.py', DIGESTS],
     f'edits_tests must name a test defined at the base: `{DIGESTS}::test_new`.'),
    (FROZEN, ['digests.py'], f'A test the change edits must be in a declared file: `{DIGESTS}`.'),
])
def test_declared_edits_name_existing_tests_in_declared_files(planner_request, digests_plan, edited, files, reason):
    digests_plan['files'] = files
    digests_plan['edits_tests'] = [edited]
    digests_plan['checks'] = [{'kind': 'demonstrating', 'node_ids': [FROZEN], 'goal_ids': [0, 1]}]
    violations = planned(planner_request, digests_plan)
    assert any(item.startswith(reason) for item in violations), violations


def test_corrected_plan_declares_its_edit(tmp_path, planner_request, digests_plan):
    digests_plan['edits_tests'] = [FROZEN]
    digests_plan['checks'] = [
        {'kind': 'demonstrating', 'node_ids': [FROZEN], 'goal_ids': [0]},
        {'kind': 'regression', 'node_ids': [SIBLING], 'goal_ids': [1]},
    ]
    assert planned(planner_request, copy.deepcopy(digests_plan)) == []
    result = run_reply(tmp_path, planner_request, digests_plan)
    assert result.exit_code == 0, result.stderr
    reply = result.events[-1]['reply']
    assert reply['edits_tests'] == [FROZEN]
    assert reply['violations'] == []


TEST_ONLY = ("A change to tests alone needs a forced reproduction (Q55) on its demonstrating check; "
             "if no forcing can be written, return can't meet.")
FORCING = {'symptom': 'Another process holds the module lock while digests import.', 'environment': {},
           'patches': [{'target': 'digests.LOCK_HELD', 'value': True}]}


def test_test_only_change_without_a_forcing_needs_a_forced_reproduction(planner_request, digests_plan):
    # PB #2625's shape: a deflake that patches only the flaky test can't demonstrate unforced (#444, Q55).
    digests_plan['files'] = [DIGESTS]
    digests_plan['edits_tests'] = [FROZEN]
    digests_plan['checks'] = [
        {'kind': 'demonstrating', 'node_ids': [FROZEN], 'goal_ids': [0]},
        {'kind': 'regression', 'node_ids': [SIBLING], 'goal_ids': [1]},
    ]
    violations = planned(planner_request, digests_plan)
    assert any('changes only tests' in item and TEST_ONLY in item for item in violations), violations
    assert not any('#393' in item for item in violations), violations


def test_test_only_change_with_a_forced_reproduction_is_accepted(planner_request, digests_plan):
    # Q55: the same deflake with a forcing on its demonstrating check is a valid plan.
    digests_plan['files'] = [DIGESTS]
    digests_plan['edits_tests'] = [FROZEN]
    digests_plan['checks'] = [
        {'kind': 'demonstrating', 'node_ids': [FROZEN], 'goal_ids': [0], 'forcing': copy.deepcopy(FORCING)},
        {'kind': 'regression', 'node_ids': [SIBLING], 'goal_ids': [1]},
    ]
    validate('planner-reply', digests_plan)
    assert planned(planner_request, copy.deepcopy(digests_plan)) == []
    # Every demonstrating check needs the forcing, and it must be valid.
    digests_plan['checks'].append({'kind': 'demonstrating', 'node_ids': [SIBLING], 'goal_ids': [0]})
    assert any(TEST_ONLY in item for item in planned(planner_request, copy.deepcopy(digests_plan)))
    digests_plan['checks'].pop()
    digests_plan['checks'][0]['forcing']['patches'][0] = {'target': 'digests.f', 'raises': 'AssertionError'}
    violations = planned(planner_request, digests_plan)
    assert any(TEST_ONLY in item for item in violations), violations
    assert any(item.startswith('Invalid forcing:') for item in violations), violations


def test_plan_schema_allows_edits_tests_only_on_a_leaf():
    validate('planner-reply', {**LEAF, 'edits_tests': [FROZEN]})
    with pytest.raises(Exception):
        validate('planner-reply', {**LEAF, 'edits_tests': [FROZEN, FROZEN]})
    with pytest.raises(Exception):
        validate('planner-reply', {**CANT_MEET, 'edits_tests': [FROZEN]})


def test_prompt_states_edits_tests_rules():
    # #438: a plan that freezes a test its goals edit burns every maker round.
    prompt = " ".join((Path(__file__).parents[1] / "src/tl3_workers/prompts/planner.md").read_text().split())
    assert "edits_tests" in prompt
    assert "never in a regression or behavior-preserving check" in prompt
    assert "naming only tests the change doesn't edit" in prompt
    assert "If the change edits only tests" in prompt and "#393" in prompt
    assert "to extend, patch, update, fix or rewrite it" in prompt


NAME = 'test_digests_imports_only_the_standard_library'


@pytest.mark.parametrize('goal', [
    f'Fix the bug. `{NAME}` must still pass unchanged.',
    f'Do not edit {NAME}.',
    f"Don't change `{NAME}`; add new tests beside it.",
    f'Keep {NAME} green.',
    f'Fix the bug so {NAME} passes.',
    f'Fix the bug, keeping {NAME} green.',
    f'`tests/{NAME}.py` passes.',
    f'Update the docs. Leave `{NAME}` unchanged.',
])
def test_goal_that_keeps_a_frozen_test_is_accepted(planner_request, digests_plan, goal):
    # #444 review: rule 7 fires only when an edit verb governs the name.
    planner_request['inputs']['ingest']['goals'][0]['description'] = goal
    assert planned(planner_request, digests_plan) == []


@pytest.mark.parametrize('goal', [
    f'Extend `{NAME}` so the module force-executes its lazy imports.',
    f'Add a case to {NAME} for the new profile.',
    f'Add a not-interchangeable pair to `{NAME}` showing the new profile differs.',
    f'Update the import assertion in `{NAME}`.',
    f'Patch {NAME} to load the module standalone.',
    f'Rewrite {DIGESTS}::{NAME} without the sleep.',
])
def test_goal_that_edits_a_frozen_test_is_refused(planner_request, digests_plan, goal):
    planner_request['inputs']['ingest']['goals'][0]['description'] = goal
    violations = planned(planner_request, digests_plan)
    assert any(f'Goal 0 names `{NAME}`' in item for item in violations), violations


def test_test_only_leaf_whose_goal_names_a_frozen_test_answers_cant_meet(planner_request, digests_plan):
    # PQ #3070 and Tessera #1532: a deflake goal names the flaky test without an edit verb.
    planner_request['inputs']['ingest']['goals'][0]['description'] = (
        f'Identify the cause of the intermittent failure of {NAME} and reproduce it deterministically.')
    digests_plan['files'] = [DIGESTS]
    digests_plan['checks'] = [{'kind': 'regression', 'node_ids': [FROZEN, SIBLING], 'goal_ids': [0, 1]}]
    violations = planned(planner_request, copy.deepcopy(digests_plan))
    assert any('changes only tests' in item and TEST_ONLY in item for item in violations), violations
    digests_plan['files'] = ['digests.py', DIGESTS]
    assert planned(planner_request, digests_plan) == []


def test_test_only_leaf_cannot_demonstrate(planner_request, digests_plan):
    # The base run applies every test-file edit, so a test-only head is the demonstrating base.
    digests_plan['files'] = [DIGESTS, 'tests/test_lazy.py']
    digests_plan['checks'] = [
        {'kind': 'demonstrating', 'node_ids': ['tests/test_lazy.py::test_lazy_imports_run'], 'goal_ids': [0]},
        {'kind': 'regression', 'node_ids': [SIBLING], 'goal_ids': [1]},
    ]
    violations = planned(planner_request, digests_plan)
    assert any('changes only tests' in item and TEST_ONLY in item for item in violations), violations


def test_test_only_regression_plan_is_accepted(planner_request, digests_plan):
    # A test refactor guarded by existing tests can pass; only shapes that can't are refused.
    planner_request['inputs']['ingest']['goals'][0]['description'] = 'Share the digest fixtures.'
    digests_plan['files'] = [DIGESTS]
    digests_plan['checks'] = [{'kind': 'regression', 'node_ids': [FROZEN, SIBLING], 'goal_ids': [0, 1]}]
    assert planned(planner_request, digests_plan) == []


def test_publication_obligations_have_guards_instead_of_pretend_tests(tmp_path, planner_request):
    planner_request['inputs']['ingest']['goals'].append({
        'description': 'Publish a non-draft PR with recorded independent proof.',
        'verification': 'review', 'stage': 'publishing',
        'publication': {'non_draft': True, 'body_lines': ['Change type: test'], 'proof': True},
    })
    result = run_reply(tmp_path, planner_request, LEAF)
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]['reply']['violations'] == []


def test_restart_plans_approved_retention_exception(tmp_path, planner_request):
    context = {"schema_version": 1, "request_version": 121, "decisions": [{
        "id": "pq2662-q10", "source_comment": "https://github.com/RobTand/prismaquant/issues/2662#issuecomment-6580255177",
        "question": {"id": 10, "kind": "cant_meet"},
        "answer": "Allow the current-source pin update.",
        "allowed_scope": ["tests/test_guarded_read_digest_retention_2637.py"],
        "restrictions": ["CURRENT_SOURCE_BLOBS only", "Preserve historical manifest material"],
        "origin_request_version": 83, "superseded_by": None,
    }]}
    planner_request["inputs"]["decision_context"] = context
    reply = copy.deepcopy(LEAF)
    reply["files"].append("tests/test_guarded_read_digest_retention_2637.py")
    result = run_reply(tmp_path, planner_request, reply)
    assert result.exit_code == 0, result.stderr
    planned = [event["reply"] for event in result.events if event["event"] == "reply"][0]
    assert "tests/test_guarded_read_digest_retention_2637.py" in planned["files"]
    assert planned["kind"] == "leaf"


def test_decision_drives_retention_plan_and_preserves_restrictions(tmp_path, planner_request, monkeypatch):
    context = {"schema_version": 1, "request_version": 121, "decisions": [{
        "id": "pq2662-q10", "source_comment": "https://github.com/RobTand/prismaquant/issues/2662#issuecomment-6580255177",
        "question": {"id": 10}, "answer": "Allow the pin update with restrictions.",
        "allowed_scope": ["tests/test_guarded_read_digest_retention_2637.py"],
        "restrictions": ["CURRENT_SOURCE_BLOBS only", "Preserve historical manifest material"],
        "origin_request_version": 83, "superseded_by": None,
    }]}
    planner_request['inputs']['decision_context'] = context
    setup = '''
class DecisionReadingModel(FakeMessagesListChatModel):
    def _generate(self, messages, **kwargs):
        inputs = json.loads(next(m.content for m in messages if m.type == 'human'))
        decisions = inputs.get('decision_context', {}).get('decisions', [])
        if not decisions:
            reply = {'kind':'cant_meet', 'report':'May I update the retention pin?', 'files':[]}
        else:
            decision = decisions[0]
            assert decision['restrictions'] == ['CURRENT_SOURCE_BLOBS only', 'Preserve historical manifest material']
            reply = BASE_LEAF.copy()
            reply['files'] = BASE_LEAF['files'] + decision['allowed_scope']
        self.responses = [AIMessage(content=json.dumps(reply))]
        return super()._generate(messages, **kwargs)
'''.replace('BASE_LEAF', repr(LEAF))
    monkeypatch.setattr(worker_process, 'LAUNCH', worker_process.LAUNCH.replace(
        'model = FakeMessagesListChatModel(', setup + '\nmodel = DecisionReadingModel('))
    result = run_reply(tmp_path, planner_request, LEAF)
    assert result.exit_code == 0, result.stderr
    plan = next(event['reply'] for event in result.events if event['event'] == 'reply')
    assert plan['kind'] == 'leaf', 'the answered scope question must not be repeated'
    assert context['decisions'][0]['allowed_scope'][0] in plan['files']
    assert plan.get('decision_context') == context, 'the plan must preserve the approved restrictions'
    del planner_request['inputs']['decision_context']
    result = run_reply(tmp_path, planner_request, LEAF)
    assert result.exit_code == 0, result.stderr
    plan = next(event['reply'] for event in result.events if event['event'] == 'reply')
    assert plan['kind'] == 'cant_meet', 'without the decision the scope question is unresolved'


@pytest.mark.parametrize('dimension,value', [
    ('executor', 'buildbuddy_remote_bazel'),
    ('runner_features', ['pinned_cpu_contention']),
    ('artifacts', ['invocation_url']),
    ('methods', ['forced_reproduction.contention_wrapper']),
    ('protected_files', ['.github/workflows/gate.yml']),
])
def test_evidence_contract_blocks_unsupported_requirements(planner_request, monkeypatch, dimension, value):
    from tl3_workers.roles.planner import check_reply
    monkeypatch.chdir(planner_request['worktree'])
    tool = {'tool': 'run_tests', 'executor': 'prismabuild_pytest',
            'runner_features': ['base_head'], 'artifacts': ['logs', 'terminal_verdict', 'sha'],
            'methods': ['red_green'], 'protected_files': []}
    requirement = {**copy.deepcopy(tool), 'requirement': 'PB2640-2650 and PB2654 require exact remote evidence.'}
    requirement[dimension] = value
    inputs = copy.deepcopy(planner_request['inputs'])
    inputs['tool_capabilities'] = [tool]
    inputs['ingest']['evidence_contract'] = [requirement]
    proposal = copy.deepcopy(LEAF)
    proposal['evidence_contract'] = []  # Planning cannot erase an intake requirement.
    check_reply(inputs, proposal)
    assert proposal['kind'] == 'cant_meet'
    assert dimension in proposal['report']
    assert (value if isinstance(value, str) else value[0]) in proposal['report']
    assert requirement in proposal['evidence_contract']


def test_evidence_contract_requires_one_tool_to_supply_all_artifacts(planner_request, monkeypatch):
    from tl3_workers.roles.planner import check_reply
    monkeypatch.chdir(planner_request['worktree'])
    inputs = copy.deepcopy(planner_request['inputs'])
    capability = {'tool': 'run_tests', 'executor': 'prismabuild_pytest',
                  'runner_features': ['base_head'], 'artifacts': ['logs', 'terminal_verdict'],
                  'methods': ['red_green'], 'protected_files': []}
    inputs['tool_capabilities'] = [dict(capability, artifacts=['logs']),
                                   dict(capability, artifacts=['terminal_verdict'])]
    inputs['ingest']['evidence_contract'] = [dict(capability, requirement='Both artifacts from the run.')]
    reply = dict(copy.deepcopy(LEAF), evidence_contract=[])
    check_reply(inputs, reply)
    assert reply['kind'] == 'cant_meet'
    inputs['tool_capabilities'] = [capability]
    reply = dict(copy.deepcopy(LEAF), evidence_contract=[])
    check_reply(inputs, reply)
    assert reply['kind'] == 'leaf'
    assert reply['violations'] == []



def test_evidence_contract_rechecks_current_capabilities(planner_request, monkeypatch):
    from tl3_workers.roles.planner import check_reply
    monkeypatch.chdir(planner_request['worktree'])
    inputs = copy.deepcopy(planner_request['inputs'])
    capability = {'tool': 'run_tests', 'executor': 'prismabuild_pytest',
                  'runner_features': ['base_head'], 'artifacts': ['logs'],
                  'methods': ['forced_reproduction.patches'], 'protected_files': []}
    inputs['tool_capabilities'] = [capability]
    inputs['ingest']['evidence_contract'] = [dict(capability, requirement='Keep the log and force the flake.')]
    inputs['ingest']['evidence_errors'] = ['Unsupported evidence requirement: artifacts=logs.']
    reply = dict(copy.deepcopy(LEAF), evidence_contract=[])
    check_reply(inputs, reply)
    assert reply['kind'] == 'leaf'
    assert reply['evidence_errors'] == []


@pytest.mark.parametrize("separate", [False, True])
def test_helper_target_observer_plan_is_supported(planner_request, digests_plan, separate):
    path = Path(planner_request['worktree']) / DIGESTS
    path.write_text(path.read_text() + '\ndef _execute(result):\n    assert result.returncode == 0\n    return result\n')
    planner_request['inputs']['ingest']['goals'] = [
        {'description': 'Improve failed helper diagnostics while the inner pytest still fails.', 'verification': 'test'}]
    observer = 'tests/test_observer.py' if separate else DIGESTS
    digests_plan.update(files=list(dict.fromkeys([DIGESTS, observer])), checks=[{
        'kind': 'demonstrating', 'node_ids': [f'{observer}::test_new_diagnostic'], 'goal_ids': [0],
        'helper_evidence': {'target': f'{DIGESTS}::_execute', 'failure_cases': [
            {'args': [{'$attributes': {'returncode': 1, 'stdout': 'broken transfer'}}], 'kwargs': {}}]}}])
    validate('planner-reply', digests_plan)
    assert planned(planner_request, copy.deepcopy(digests_plan)) == []
    digests_plan['checks'][0]['helper_evidence']['target'] = f'{DIGESTS}::missing'
    digests_plan['files'].append('README.md')
    assert any('helper evidence' in item for item in planned(planner_request, copy.deepcopy(digests_plan)))


def test_helper_preflight_requires_installed_target_observer_capability():
    from tl3_workers.evidence import preflight
    capability = {'tool': 'run_tests', 'executor': 'prismabuild_pytest', 'runner_features': ['base_head'],
                  'artifacts': ['cas_receipt', 'terminal_verdict'], 'methods': [], 'protected_files': []}
    reply = {'kind': 'leaf', 'files': ['tests/test_helpers.py'], 'evidence_contract': [],
             'checks': [{'helper_evidence': {'target': 'tests/test_helpers.py::_execute'}}]}
    original = copy.deepcopy(reply)
    preflight({'tool_capabilities': [capability]}, reply, planning=True)
    assert reply['kind'] == 'cant_meet'
    assert 'methods=helper_target_observer' in reply['report']
    capability['methods'] = ['helper_target_observer']
    preflight({'tool_capabilities': [capability]}, original, planning=True)
    assert original['kind'] == 'leaf' and original['evidence_errors'] == []
