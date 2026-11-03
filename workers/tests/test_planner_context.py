"""Planner context contracts."""
# ruff: noqa: F811

import copy

import pytest
from jsonschema import ValidationError

from tl3_workers.contracts import validate
import worker_process
from planner_fixtures import (
    CANT_MEET,
    FLAG,
    LEAF,
    MEMORIES,
    cache_tokenizer,  # noqa: F401
    planner_request,  # noqa: F401
    run_reply,
    tool_capable_fake,  # noqa: F401
)


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
