"""Prompts must state the declared-file limits the code enforces (#321 follow-up).

PQ #2602 and #2629 stalled because planner.md still said declared files over
1,048,576 bytes fail the run after forecast.py stopped enforcing that limit.
"""

from importlib.resources import files
import re

import pytest

from tl3_workers import forecast

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
