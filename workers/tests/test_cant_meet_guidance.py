"""The maker is told when cant_meet applies (#434). PQ #2629 attempt 2 spent a credit on one for
steps its own tools could do."""

from importlib.resources import files
import json
import re

from tl3_workers.roles import ROLES


def text(*parts):
    return " ".join(files("tl3_workers").joinpath(*parts).read_text().split())


def test_the_maker_prompt_says_when_cant_meet_applies():
    prompt = text("prompts", "maker.md")
    assert ("Use cant_meet only when no change within the declared files and your own tools can meet "
            "a goal: a plan defect, a rule conflict, tests that run and still fail after your best fix, "
            "or evidence only the controller or a person can supply") in prompt
    assert "Unfinished steps that your own tools can do are not cant_meet: do them" in prompt
    assert ("List items you think are met but could not re-verify in done's account, marked "
            "unverified; the controller's checks and the independent review judge them after done") in prompt
    assert "When your context is nearly full, finish the remaining tool steps first, then reply" in prompt


def test_the_maker_prompt_no_longer_offers_cant_meet_for_any_unmet_goal():
    prompt = text("prompts", "maker.md")
    assert "If any goal cannot be met, return cant_meet with a report explaining why" not in prompt
    assert prompt.count("Use cant_meet when the change cannot meet a goal") == 0


def test_the_tools_the_prompt_calls_the_makers_own_are_the_tools_it_has():
    prompt = text("prompts", "maker.md")
    named, = re.findall(r"Your own tools are ([a-z_, ]+?)\.", prompt)
    assert set(re.split(r", | and ", named)) == set(ROLES["maker"].tools)


def test_the_cant_meet_schema_description_matches_the_prompt():
    schema = json.loads(files("tl3_workers").joinpath("schemas", "maker-reply.json").read_text())
    variant = next(item for item in schema["oneOf"] if item["properties"]["kind"]["const"] == "cant_meet")
    description = " ".join(variant["description"].split())
    assert ("Only when no change within the declared files and your own tools can meet a goal, or the "
            "plan is defective. Not for unfinished steps your own tools can do.") == description
