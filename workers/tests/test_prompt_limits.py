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
GREP_PATTERN = "grep's pattern is a Python regular expression"
GREP_LITERAL = "set literal to true to search text as written"
GLOB_PATH = "glob's path must be a directory"
SHARED_TOOLS = ("grep", "glob")
# Every grep argument that is not self-explanatory must be named wherever grep is described.
GREP_OPTIONS = sorted(set(inspect.signature(Worktree.grep).parameters) - {"self", "pattern", "path", "offset"})


@pytest.mark.parametrize("name", ["maker.md", "planner.md", "reviewer.md"])
def test_prompts_describe_the_path_argument_of_grep_and_glob(name):
    prompt = prompt_text(name)
    assert GREP_PATH in prompt
    assert GREP_PATTERN in prompt
    assert GREP_LITERAL in prompt
    assert GLOB_PATH in prompt
    assert "not a regular expression" not in prompt
    for option in GREP_OPTIONS:
        assert option in prompt


@pytest.mark.parametrize("role", sorted(ROLES))
def test_role_tool_descriptions_describe_the_path_argument(role):
    tools = ROLES[role].tools
    if "grep" not in tools:
        pytest.skip(f"{role} has no file tools")
    assert "path may be a file or a directory" in tools["grep"]
    assert "pattern is a Python regular expression" in tools["grep"]
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
