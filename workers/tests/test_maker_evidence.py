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
import message_invariants
from pb_fixtures import FLUX_EXECUTION, KEY, run_binding
from pb_fixtures import supported_fake_runtime as supported_fake_runtime
from message_invariants import RUN_FIVE as RUN_FIVE
from message_invariants import git as git
from message_invariants import maker_request as maker_request
from message_invariants import receipts_factory as receipts_factory
from message_invariants import request_123_handover as request_123_handover
from message_invariants import tool_capable_process_model as tool_capable_process_model
from message_invariants import turn as turn
from message_invariants import write_turn as write_turn


# The native receipt flux_execution.execute builds from a verified terminal.
RECEIPT = {"backend": "flux", "wait_reference": KEY, "run_binding": run_binding(KEY),
           "receipt_sha256": "b" * 64, "ending": "passed"}


@pytest.fixture
def native_receipts(monkeypatch):
    monkeypatch.setattr(message_invariants, "RECEIPT", RECEIPT)


def flux_maker(maker_request, supported_fake_runtime, summary):
    maker_request["inputs"]["tests"] = {"image": maker_request["inputs"]["tests"]["image"],
                                        "execution": dict(FLUX_EXECUTION)}
    (supported_fake_runtime.state / "response.json").write_text(json.dumps(summary))
    return maker_request


def test_done_reply_carries_the_run_tests_receipts(maker_request, native_receipts, monkeypatch):
    maker_request["inputs"]["context_size"] = 150_000
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
    assert evidence["receipts"] == [RECEIPT] and "fresh" not in evidence
    assert evidence["tree"] == git(Path(maker_request["worktree"]), "rev-parse", f"{reply['head']}^{{tree}}")
    validate("maker-reply", reply)


def test_runs_before_a_later_edit_are_not_evidence_for_the_head(maker_request, native_receipts, monkeypatch):
    maker_request["inputs"]["context_size"] = 150_000
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
             "status": "completed", "tree": "e" * 40,
             "totals": {"passed": 1, "failed": 0, "error": 0, "skipped": 0}, "receipts": [RECEIPT]}
    done = {"kind": "done", "head": "f" * 40, "self_check": DONE["self_check"]}
    validate("maker-reply", {**done, "test_evidence": [entry]})
    for evidence in ([entry] * 11, [{**entry, "tests": ["t" * 2049]}], [{**entry, "tests": ["t"] * 257}],
                     [{**entry, "receipts": [RECEIPT] * 2}], [{**entry, "tree": "not a tree"}],
                     [{**entry, "fresh": True}],
                     [{**entry, "receipts": [{**RECEIPT, "status": "executed"}]}],
                     [{**entry, "receipts": [{**RECEIPT, "backend": "prismabuild"}]}],
                     [{**entry, "receipts": [{**RECEIPT, "wait_reference": "a" * 64}]}],
                     [{**entry, "receipts": [{**RECEIPT, "receipt_sha256": "B" * 64}]}],
                     [{**entry, "receipts": [{**RECEIPT, "ending": "succeeded"}]}],
                     [{**entry, "receipts": [{key: value for key, value in RECEIPT.items() if key != "run_binding"}]}],
                     [{**entry, "receipts": [{**RECEIPT, "run_binding": {"id": RECEIPT["run_binding"]["id"]}}]}],
                     [{**entry, "receipts": [{**RECEIPT, "run_binding": {**RECEIPT["run_binding"], "id": {
                         **RECEIPT["run_binding"]["id"], "job": "a" * 64}}}]}],
                     [{**entry, "receipts": [{**RECEIPT, "log_sha256": "c" * 64}]}]):
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
    assert "Make your fix pass with and without that forcing" in prompt


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


def test_flux_run_tests_capture_reaches_canonical_done_admission(maker_request, supported_fake_runtime, monkeypatch):
    # Real run_tests: staging, flux_execution.execute, payload blob checks, native receipt,
    # Evidence.test_result capture, maker done reply and the canonical maker-reply schema.
    summary = {"status": "completed", "tests": [{"test": "test_greeting.py::test_hello", "outcome": "passed",
                                                 "message": ""}],
               "totals": {"passed": 1, "failed": 0, "error": 0, "skipped": 0}, "note": ""}
    maker_request = flux_maker(maker_request, supported_fake_runtime, summary)
    maker_request["inputs"]["context_size"] = 150_000
    fake = FakeMessagesListChatModel(responses=[
        AIMessage(**write_turn()),
        AIMessage(**write_turn("test_greeting.py", "def test_hello(): pass\n")),
        RUN_FIVE,
        AIMessage(**turn(DONE)),
    ])
    monkeypatch.setattr(runtime, "create_model", lambda request: fake)
    events = []
    runtime.run(maker_request, events.append)
    reply = events[-1]["reply"]
    assert reply["kind"] == "done", reply
    [evidence] = reply["test_evidence"]
    assert evidence["receipts"] == [RECEIPT]
    assert "fresh" not in evidence
    assert supported_fake_runtime.submitted == [KEY]
    assert evidence["tree"] == git(Path(maker_request["worktree"]), "rev-parse", f"{reply['head']}^{{tree}}")
    validate("maker-reply", reply)


@pytest.mark.parametrize("cited", ["recorded", "unrecorded", "pb_key"])
def test_flux_infrastructure_wait_reference_backs_only_its_own_blocked_reply(
        maker_request, supported_fake_runtime, monkeypatch, cited):
    maker_request = flux_maker(maker_request, supported_fake_runtime, {
        "status": "completed", "tests": [], "totals": {"passed": 0, "failed": 0, "error": 0, "skipped": 0},
        "note": ""})
    maker_request["inputs"]["context_size"] = 150_000
    supported_fake_runtime.ending = "infra-lost"
    reference = {"recorded": KEY, "unrecorded": "worker:" + "d" * 64, "pb_key": "a" * 64}[cited]
    blocked = {"kind": "blocked", "evidence": {"wait_reference": reference},
               "report": "The run backend lost the test run."}
    fake = FakeMessagesListChatModel(responses=[RUN_FIVE, AIMessage(**turn(blocked)), AIMessage(**turn(blocked))])
    monkeypatch.setattr(runtime, "create_model", lambda request: fake)
    events = []
    if cited == "recorded":
        runtime.run(maker_request, events.append)
        reply = events[-1]["reply"]
        assert reply["kind"] == "blocked", reply
        assert reply["evidence"]["wait_reference"] == KEY
        assert reply["evidence"]["class"] == "test_infrastructure"
        validate("maker-reply", reply)
    elif cited == "unrecorded":
        runtime.run(maker_request, events.append)
        reply = events[-1]["reply"]
        assert reply["kind"] == "cant_meet", "an unrecorded reference never backs a blocked reply"
        assert "The run backend lost the test run." in reply["report"]
    else:
        from tl3_workers.turns import InvalidReply
        with pytest.raises(InvalidReply):
            runtime.run(maker_request, events.append)
        assert not any(event.get("event") == "reply" for event in events)
