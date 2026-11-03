# ruff: noqa: F811

import json
from pathlib import Path
from jsonschema import ValidationError
from langchain_core.language_models.fake_chat_models import FakeMessagesListChatModel
from langchain_core.messages import AIMessage
import pytest
from tl3_workers import runtime
from tl3_workers.contracts import validate
from tl3_workers.roles import maker
from tl3_workers.tools import TOOL_FACTORIES
from worker_process import start_worker
from message_invariants import DONE as DONE
from message_invariants import RECEIPT as RECEIPT
from message_invariants import RUN_FIVE as RUN_FIVE
from message_invariants import git as git
from message_invariants import maker_request as maker_request
from message_invariants import receipts_factory as receipts_factory
from message_invariants import request_123_handover as request_123_handover
from message_invariants import tool_capable_process_model as tool_capable_process_model
from message_invariants import turn as turn
from message_invariants import write_turn as write_turn


def test_done_reply_carries_the_run_tests_receipts(maker_request, monkeypatch):
    # #408: the evidence the maker cites lands in its reply for the reviewer,
    # for the tree that run tested, which is the committed head's tree (#432).
    fake = FakeMessagesListChatModel(responses=[
        AIMessage(**write_turn()),
        AIMessage(**write_turn("test_greeting.py", "def test_hello(): pass\n")),
        RUN_FIVE,
        AIMessage(**turn(DONE)),
    ])
    monkeypatch.setitem(TOOL_FACTORIES, "run_tests", receipts_factory)
    monkeypatch.setattr(runtime, "create_model", lambda request: fake)
    events = []
    runtime.run(maker_request, events.append)
    reply = events[-1]["reply"]
    assert reply["kind"] == "done"
    [evidence] = reply["test_evidence"]
    assert evidence["tests"] == ["test_greeting.py::test_hello"]
    assert evidence["repeat"] == 5 and len(evidence["runs"]) == 5
    assert evidence["receipts"] == [RECEIPT] and evidence["fresh"] is True
    assert evidence["tree"] == git(Path(maker_request["worktree"]), "rev-parse", f"{reply['head']}^{{tree}}")
    validate("maker-reply", reply)


def test_runs_before_a_later_edit_are_not_evidence_for_the_head(maker_request, monkeypatch):
    # #432: run, edit, done. The run tested a tree the head no longer has.
    root = Path(maker_request["worktree"])
    maker_request["inputs"]["check_base_sha"] = git(root, "rev-parse", "HEAD")
    fake = FakeMessagesListChatModel(responses=[
        AIMessage(**write_turn("test_greeting.py", "def test_hello(): pass\n")),
        RUN_FIVE,
        AIMessage(**write_turn()),
        AIMessage(**turn(DONE)),
    ])
    monkeypatch.setitem(TOOL_FACTORIES, "run_tests", receipts_factory)
    monkeypatch.setattr(runtime, "create_model", lambda request: fake)
    events = []
    runtime.run(maker_request, events.append)
    reply = events[-1]["reply"]
    assert reply["kind"] == "done"
    assert "test_evidence" not in reply
    assert reply["checking_handoff"]["head_tree"] == git(root, "rev-parse", reply["head"] + "^{tree}")
    assert reply["checking_handoff"]["status"] == "pending"


@pytest.mark.parametrize("special", [{"forcing": {"symptom": "Q55"}}])
def test_ordinary_handoff_never_substitutes_for_forced_proof(maker_request, special):
    inputs = maker_request["inputs"]
    inputs["check_base_sha"] = "a" * 40
    inputs["plan"]["checks"][0].update(special)
    assert maker._checking_handoff(inputs, "b" * 40, "c" * 40) is None


def test_test_evidence_is_bounded():
    # #432: entries, selectors and strings are capped, and the worker keeps the whole list small.
    from tl3_workers.tools import TEST_EVIDENCE_BYTES, Evidence

    entry = {"tests": ["test_greeting.py::test_hello"], "base": False, "repeat": 1, "parallel": 0,
             "status": "completed", "tree": "e" * 40, "fresh": True,
             "totals": {"passed": 1, "failed": 0, "error": 0, "skipped": 0}, "receipts": [RECEIPT]}
    done = {"kind": "done", "head": "f" * 40, "self_check": DONE["self_check"]}
    validate("maker-reply", {**done, "test_evidence": [entry]})
    for evidence in ([entry] * 11, [{**entry, "tests": ["t" * 2049]}], [{**entry, "tests": ["t"] * 257}],
                     [{**entry, "receipts": [RECEIPT] * 2}], [{**entry, "tree": "not a tree"}]):
        with pytest.raises(ValidationError):
            validate("maker-reply", {**done, "test_evidence": evidence})
    recorder = Evidence()
    for index in range(10):
        tests = tuple(f"test_{index}_{n}_" + "x" * 1900 for n in range(256))
        recorder.test_result({"status": "completed", "totals": entry["totals"], "receipts": [RECEIPT],
                              "tree": "e" * 40}, (False, tests), {"repeat": 1, "parallel": 0})
    kept = recorder.test_evidence()
    assert kept and len(json.dumps(kept).encode()) <= TEST_EVIDENCE_BYTES
    assert kept[-1]["tests"][0].startswith("test_9_"), "the newest run is kept"
    assert kept[-1]["tests_omitted"] == 256 - len(kept[-1]["tests"]) > 0
    validate("maker-reply", {**done, "test_evidence": kept})


def test_maker_input_carries_a_forced_check(maker_request):
    # #393: the maker sees the forcing its deflake must survive.
    check = maker_request['inputs']['plan']['checks'][0]
    check['forcing'] = {"symptom": "A real, unrelated process holds PID 4242.", "environment": {},
                        "patches": [{"target": "sampler.process_in_scope", "raises": "ValueError"}]}
    validate('maker-input', maker_request['inputs'])
    check['forcing']['code'] = 'print(1)'
    with pytest.raises(ValidationError):
        validate('maker-input', maker_request['inputs'])


def test_maker_prompt_explains_forced_checks():
    prompt = " ".join((Path(__file__).parents[1] / "src/tl3_workers/prompts/maker.md").read_text().split())
    assert "forcing" in prompt
    assert "both with and without the forcing" in prompt


def test_request_123_honest_handover_with_downstream_pending(request_123_handover):
    request, reply = request_123_handover
    maker._check_inputs(request["inputs"])
    maker._check_reply(request["inputs"], reply)


def test_request_123_pending_handover_commits_through_real_worker(tmp_path, request_123_handover):
    request, reply = request_123_handover
    del request["inputs"]["plan"]["checks"][0]["forcing"]
    result = start_worker(tmp_path, request, [write_turn("test_greeting.py", "def test_hello(): assert True\n"), turn(reply)])
    assert result.exit_code == 0, result.stderr
    assert result.events[-1]["reply"]["kind"] == "done"
    assert result.events[-1]["reply"]["self_check"] == reply["self_check"]


@pytest.mark.parametrize("goal_id", ["repair", "scope"])
def test_request_123_maker_cannot_defer_implementation_or_scope(request_123_handover, goal_id):
    request, reply = request_123_handover
    for check in reply["self_check"]:
        if check["goal_id"] == goal_id:
            check.clear()
            check.update(goal_id=goal_id, status="pending", why="Calling this downstream does not change ownership.")
    with pytest.raises(ValueError, match="maker"):
        maker._check_reply(request["inputs"], reply)


def test_request_123_maker_cannot_claim_future_proof(request_123_handover):
    request, reply = request_123_handover
    reply["self_check"][-1] = {"goal_id": "publication", "status": "met", "evidence": "Imaginary PR."}
    with pytest.raises(ValueError, match="pending"):
        maker._check_reply(request["inputs"], reply)
