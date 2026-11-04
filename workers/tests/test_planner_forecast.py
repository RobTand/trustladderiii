"""Planner forecast contracts."""
# ruff: noqa: F811

import copy
import os

import pytest
import tiktoken

from tl3_workers.contracts import validate
from planner_fixtures import (
    CANT_MEET,
    LEAF,
    SPLIT,
    assert_rejected,
    assert_rule_report,
    cache_tokenizer,  # noqa: F401
    planner_request,  # noqa: F401
    run_reply,
    tool_capable_fake,  # noqa: F401
)


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
    assert {key: value for key, value in reply.items() if key not in {"violations", "execution"}} == {**proposal, "forecast": {
        "tokenizer": "o200k_base", "brief_tokens": 2, "file_tokens": files,
        "overhead_tokens": 60, "tokens": 62 + sum(files.values()),
        "fit_tokens": 100000, "context_tokens": 150000, "fits": True,
    }}
    validate("planner-reply", reply)


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
