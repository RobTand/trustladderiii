"""#321: large or unusable declared files never crash a forecast."""

import copy
import json

import pytest

from test_planner import LEAF, SPLIT, cache_tokenizer, planner_request, run_reply, tool_capable_fake  # noqa: F401
from test_workers import BRIEF, request_doc  # noqa: F401
from tl3_workers.forecast import RANGED_FILE_TOKENS
from tl3_workers.tools import Worktree
from worker_process import start_worker

ARCHITECTURE = b"# Architecture\n\n" + b"The quantizer packs weights per channel.\n" * 62_000  # 2.5 MB


def ingest(tmp_path, request_doc, files):  # noqa: F811
    request_doc["inputs"]["forecast"] = {
        "fit_tokens": 100_000, "context_tokens": 150_000, "overhead_tokens": {"prompt": 10},
    }
    reply = {**BRIEF, "files": files}
    return start_worker(tmp_path, request_doc, [{"content": json.dumps(reply)}])


def test_ingest_forecasts_a_large_markdown_file_within_the_fit_limit(tmp_path, request_doc):  # noqa: F811
    (tmp_path / "docs").mkdir()
    (tmp_path / "docs/ARCHITECTURE.md").write_bytes(ARCHITECTURE)
    result = ingest(tmp_path, request_doc, ["docs/ARCHITECTURE.md"])
    assert result.exit_code == 0, result.stderr
    reply = result.events[-1]["reply"]
    assert reply["forecast"]["file_tokens"] == {"docs/ARCHITECTURE.md": RANGED_FILE_TOKENS}
    assert reply["forecast"]["fits"] is True
    # Worker code tells the maker how to work on it.
    assert "docs/ARCHITECTURE.md" in reply["brief"] and str(len(ARCHITECTURE)) in reply["brief"]
    assert "by range" in reply["brief"] and "edit" in reply["brief"]


@pytest.mark.parametrize("content,reason", [
    (b"\x89PNG\r\n\x1a\n\x00\x00binary", "binary"),
    (b"\xff\xfe\x00not utf-8", "binary"),
])
def test_ingest_reports_an_unusable_file_in_its_reply_instead_of_crashing(tmp_path, request_doc, content, reason):  # noqa: F811
    (tmp_path / "logo.png").write_bytes(content)
    result = ingest(tmp_path, request_doc, ["logo.png"])
    # A reply, not a crash: a retry would fail the same way, so it isn't an infrastructure failure.
    assert result.exit_code == 0, result.stderr
    reply = result.events[-1]["reply"]
    assert reply["forecast"]["file_tokens"] == {"logo.png": 0}
    assert f"logo.png: {reason}" in reply["brief"] and f"({len(content)} bytes)" in reply["brief"]


def test_planner_counts_a_large_file_as_a_ranged_edit(tmp_path, planner_request):  # noqa: F811
    (tmp_path / "worktree/greeting.py").write_bytes(ARCHITECTURE)
    result = run_reply(tmp_path, planner_request, LEAF)
    assert result.exit_code == 0, result.stderr
    reply = result.events[-1]["reply"]
    assert reply["forecast"]["file_tokens"]["greeting.py"] == RANGED_FILE_TOKENS
    assert reply["violations"] == []


@pytest.mark.parametrize("content,reason", [
    (b"hello\x00world", "binary"),
    (b"x" * (16 * 1_048_576 + 1), "over the 16777216-byte limit"),
], ids=["binary", "over_cap"])
def test_planner_names_an_unusable_file_as_a_violation(tmp_path, planner_request, content, reason):  # noqa: F811
    (tmp_path / "worktree/greeting.py").write_bytes(content)
    result = run_reply(tmp_path, planner_request, LEAF)
    assert result.exit_code == 0, result.stderr
    violations = result.events[-1]["reply"]["violations"]
    assert any(f"greeting.py: {reason}" in violation and f"({len(content)} bytes)" in violation
               for violation in violations), violations


def test_planner_names_unusable_files_in_split_pieces(tmp_path, planner_request):  # noqa: F811
    (tmp_path / "worktree/blob.bin").write_bytes(b"\x00\x01\x02")
    proposal = copy.deepcopy(SPLIT)
    proposal["pieces"][1]["files"] = ["blob.bin"]
    result = run_reply(tmp_path, planner_request, proposal)
    assert result.exit_code == 0, result.stderr
    assert any("blob.bin: binary (3 bytes)" in violation
               for violation in result.events[-1]["reply"]["violations"])


def test_read_tool_reads_a_large_file_by_range(tmp_path):
    (tmp_path / "big.md").write_bytes(ARCHITECTURE)
    with Worktree(tmp_path) as worktree:
        assert worktree.read("big.md", offset=50_000, limit=2) == (
            "The quantizer packs weights per channel.\n" * 2
            + "\n[Truncated. Continue with offset=50002, column=0.]")


POOL = "".join(f"def handler_{i}(job):\n    return job.run({i})\n\n\n" for i in range(36_000)).encode()  # 1.3 MB


def test_planner_counts_a_large_python_module_as_a_ranged_edit(tmp_path, planner_request):  # noqa: F811
    # PrismaBuild's src/prismabuild/pool.py is 1.25 MB of source that many requests touch.
    (tmp_path / "worktree/greeting.py").write_bytes(POOL)
    result = run_reply(tmp_path, planner_request, LEAF)
    assert result.exit_code == 0, result.stderr
    reply = result.events[-1]["reply"]
    assert reply["forecast"]["file_tokens"]["greeting.py"] == RANGED_FILE_TOKENS
    assert reply["violations"] == []


def test_maker_tools_read_and_edit_a_large_python_module_by_range(tmp_path):
    from tl3_workers.tools import OUTPUT_LIMITS, build_tools

    assert len(POOL) > 1_300_000
    (tmp_path / "pool.py").write_bytes(POOL)
    with Worktree(tmp_path) as worktree:
        tools = {tool.name: tool for tool in build_tools(
            worktree, {"read": "Read.", "grep": "Grep.", "edit": "Edit."})}
        found = tools["grep"].invoke({"pattern": "def handler_35000("})
        assert found["matches"] == [{"path": "pool.py", "line": 140_001, "text": "def handler_35000(job):"}]
        text = tools["read"].invoke({"path": "pool.py", "offset": 140_000, "limit": 2})
        assert text.startswith("def handler_35000(job):\n    return job.run(35000)\n")
        assert len(text.encode()) <= OUTPUT_LIMITS["bytes"]
        # The whole-file read is bounded too: the model only ever sees one page.
        page = tools["read"].invoke({"path": "pool.py"})
        assert len(page.encode()) <= OUTPUT_LIMITS["bytes"] and "Truncated" in page
        result = tools["edit"].invoke({"path": "pool.py", "old": "return job.run(35000)",
                                       "new": "return job.run(35000, retry=True)"})
        assert result == "Replaced 1 occurrence(s)."
    content = (tmp_path / "pool.py").read_bytes()
    assert b"return job.run(35000, retry=True)" in content
    assert len(content) == len(POOL) + len(b", retry=True)") - 1


def test_planner_fit_check_bounds_a_large_doc_in_a_small_plan(tmp_path, planner_request):  # noqa: F811
    # Tessera #1464: a 716 KB docs/ARCHITECTURE.md with two small files forecast past the
    # ~100k fit bound and asked Rob to narrow the request. A region edit must fit.
    planner_request["inputs"]["forecast"]["fit_tokens"] = 100_000
    worktree = tmp_path / "worktree"
    (worktree / "docs").mkdir()
    (worktree / "docs/ARCHITECTURE.md").write_bytes(
        b"# Architecture\n\n" + b"Each stage hands its batch to the next queue.\n" * 15_900)
    assert (worktree / "docs/ARCHITECTURE.md").stat().st_size > 716_000
    (worktree / "notes.py").write_text("NOTE = 'small'\n")
    leaf = copy.deepcopy(LEAF)
    leaf["files"] = ["docs/ARCHITECTURE.md", "greeting.py", "notes.py"]
    result = run_reply(tmp_path, planner_request, leaf)
    assert result.exit_code == 0, result.stderr
    reply = result.events[-1]["reply"]
    assert reply["forecast"]["fits"] is True, reply["forecast"]
    assert reply["forecast"]["tokens"] < 100_000
    assert not any("does not fit" in violation for violation in reply["violations"])
