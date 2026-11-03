"""Prompts must state the declared-file limits the code enforces (#321 follow-up).

PQ #2602 and #2629 stalled because planner.md still said declared files over
1,048,576 bytes fail the run after forecast.py stopped enforcing that limit.
"""

from importlib.resources import files
import inspect
import re

import pytest

from tl3_workers import forecast
from tl3_workers.roles import ROLES
from tl3_workers.tools import Worktree, build_tools

PROMPTS = sorted(entry.name for entry in files("tl3_workers").joinpath("prompts").iterdir()
                 if entry.name.endswith(".md"))


def prompt_text(name):
    return " ".join(files("tl3_workers").joinpath("prompts", name).read_text().split())


def test_planner_states_the_current_declared_file_limits():
    prompt = prompt_text("planner.md")
    assert f"{forecast.LARGE_FILE_BYTES:,} bytes" in prompt
    assert f"{forecast.HARD_CAP_BYTES:,} bytes" in prompt
    assert f"{forecast.RANGED_FILE_TOKENS:,} tokens" in prompt


@pytest.mark.parametrize("name", PROMPTS)
def test_every_stated_byte_limit_is_one_the_code_enforces(name):
    enforced = {f"{limit:,}" for limit in (forecast.LARGE_FILE_BYTES, forecast.HARD_CAP_BYTES)}
    for number in re.findall(r"(\d[\d,]*) bytes", prompt_text(name)):
        assert number in enforced, f"{name} states a {number}-byte limit the code does not enforce"


# What the code does with `path` (see test_file_tools.py), as prompts and tool descriptions say it.
GREP_PATH = "grep's path may be a file or a directory"
GREP_PATTERN = "grep's pattern is an RE2 regular expression, with no backreferences or lookarounds"
GREP_LITERAL = "set literal to true to search text as written"
GLOB_PATH = "glob's path must be a directory"
SHARED_TOOLS = ("grep", "glob")
# Every grep argument that is not self-explanatory must be named wherever grep is described.
GREP_OPTIONS = sorted(set(inspect.signature(Worktree.grep).parameters) - {"self", "pattern", "path", "offset"})


# #648: the maker and reviewer prompts leave grep and glob to their tool descriptions, which
# test_role_tool_descriptions_describe_the_path_argument checks.
@pytest.mark.parametrize("name", ["planner.md"])
def test_prompts_describe_the_path_argument_of_grep_and_glob(name):
    prompt = prompt_text(name)
    assert GREP_PATH in prompt
    assert GREP_PATTERN in prompt
    assert GREP_LITERAL in prompt
    assert GLOB_PATH in prompt
    assert "Python" not in GREP_PATTERN and "not a regular expression" not in prompt
    for option in GREP_OPTIONS:
        assert option in prompt


@pytest.mark.parametrize("role", sorted(ROLES))
def test_role_tool_descriptions_describe_the_path_argument(role):
    tools = ROLES[role].tools
    if "grep" not in tools:
        pytest.skip(f"{role} has no file tools")
    assert "path may be a file or a directory" in tools["grep"]
    assert "pattern is an RE2 regular expression, with no backreferences or lookarounds" in tools["grep"]
    assert "set literal to true to search text as written" in tools["grep"]
    assert "not a regular expression" not in tools["grep"]
    assert "path must be a directory" in tools["glob"]
    for option in GREP_OPTIONS:
        assert option in tools["grep"]


def test_what_the_prompts_say_about_path_is_what_the_tools_do(tmp_path):
    (tmp_path / "pkg").mkdir()
    (tmp_path / "pkg/mod.py").write_text("needle\n")
    with Worktree(tmp_path) as worktree:
        tools = {tool.name: tool for tool in build_tools(
            worktree, {name: f"{name}." for name in SHARED_TOOLS})}
        for path in ("pkg", "pkg/mod.py"):
            assert [match["path"] for match in tools["grep"].invoke(
                {"pattern": "needle", "path": path})["matches"]] == ["pkg/mod.py"]
        assert tools["glob"].invoke({"pattern": "*.py", "path": "pkg"})["entries"] == ["pkg/mod.py"]
        failed = tools["glob"].invoke({"name": "glob", "id": "x", "type": "tool_call",
                                       "args": {"pattern": "*.py", "path": "pkg/mod.py"}})
        assert failed.status == "error"
        # The pattern is a regular expression, and literal true searches the text as written.
        assert tools["grep"].invoke({"pattern": "need|hay"})["matches"][0]["path"] == "pkg/mod.py"
        assert tools["grep"].invoke({"pattern": "need|hay", "literal": True})["matches"] == []


def test_run_tests_options_say_what_the_tool_enforces():
    # #408: prompts and the tool description state the bounds pb_tests enforces.
    from importlib.resources import files

    def text(role):
        return " ".join(files("tl3_workers").joinpath("prompts", f"{role}.md").read_text().split())

    description = ROLES["maker"].tools["run_tests"]
    for statement in ("repeat 1 to 10", "parallel 2 to 4", "priority -10 to 0"):
        assert statement in description, statement
    assert "receipts" in text("maker")
    assert "repeat" in text("planner") and "parallel" in text("planner") and "receipts" in text("planner")
    assert "test_evidence" in text("reviewer")


@pytest.mark.parametrize("name", ["maker.md", "reviewer.md"])
def test_saturation_guidance_bounds_a_passing_check(name):
    assert 'A saturation receipt means "PASS covers only the checked prefix"' in prompt_text(name)


CONFLICT_MARKER = re.compile(r"^(<{7} |={7}$|>{7} )", re.M)


@pytest.mark.parametrize("name", PROMPTS)
def test_no_prompt_carries_unresolved_merge_conflict_markers(name):
    # Audit and ingest shipped both sides of a #612/#607 merge conflict, markers and all.
    raw = files("tl3_workers").joinpath("prompts", name).read_text()
    assert not CONFLICT_MARKER.search(raw), f"{name} contains unresolved merge-conflict markers"
