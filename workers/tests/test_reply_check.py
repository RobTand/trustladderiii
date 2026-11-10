"""Every Rust diagnostic reaches the same bounded model repair turn."""

import json
import subprocess
from types import SimpleNamespace

import pytest

from tl3_workers import reply_check
from tl3_workers.ingest_retention import preserve_verbatim
from tl3_workers.turns import REPAIR_MESSAGE_BYTES, TurnControl


def validator(monkeypatch, violations):
    monkeypatch.setattr(reply_check, "run_check", lambda *a, **k: {"violations": violations})


@pytest.mark.parametrize("role", ["classifier", "reviewer", "postmortem", "ingest", "audit"])
def test_every_reply_diagnostic_reaches_model_in_order(monkeypatch, role):
    validator(monkeypatch, ["rule.first: first fix", "rule.second: deuxième\nfix", "rule.first: first fix"])
    with pytest.raises(reply_check.ReplyViolation) as caught:
        reply_check.check_reply(role, {}, {})
    history = []
    with TurnControl().repair(history, caught.value):
        pass
    assert "rule.first: first fix\nrule.second: deuxième\nfix" in history[0].content
    assert history[0].content.count("rule.first") == 1
    assert reply_check.failure_class(caught.value) == "ReplyViolation"


@pytest.mark.parametrize("kind,expected,recorded", [
    ("maker", reply_check.MakerInputViolation, "ValueError"),
    ("request", ValueError, "ValueError"),
])
def test_every_input_diagnostic_keeps_exception_class(monkeypatch, kind, expected, recorded):
    validator(monkeypatch, ["first", "second", "first"])
    with pytest.raises(expected) as caught:
        reply_check.check_inputs(kind, {})
    assert str(caught.value) == "first\nsecond"
    assert reply_check.failure_class(caught.value) == recorded


def test_many_diagnostics_keep_prefix_and_count_omissions(monkeypatch):
    rows = [f"rule.{i}: " + "é" * 100 for i in range(100)]
    validator(monkeypatch, rows)
    with pytest.raises(reply_check.ReplyViolation) as caught:
        reply_check.check_reply("reviewer", {}, {})
    text = str(caught.value)
    assert len(text.encode()) <= REPAIR_MESSAGE_BYTES
    shown = sum(row in text for row in rows)
    assert shown > 1
    assert text.startswith("\n".join(rows[:shown]))
    assert text.endswith(f"[{len(rows) - shown} more violations omitted]")
    history = []
    with TurnControl().repair(history, caught.value):
        pass
    assert text in history[0].content


def test_oversized_first_diagnostic_is_counted_and_keeps_its_start(monkeypatch):
    validator(monkeypatch, ["first: " + "界" * 3000, "second"])
    with pytest.raises(reply_check.ReplyViolation) as caught:
        reply_check.check_reply("classifier", {}, {})
    text = str(caught.value)
    assert len(text.encode()) <= REPAIR_MESSAGE_BYTES
    assert text.startswith("first: 界")
    assert "[1 violation truncated; 1 more violations omitted]" in text


@pytest.mark.parametrize("call", [
    lambda: reply_check.check_reply("reviewer", {}, {}),
    lambda: reply_check.check_inputs("maker", {}),
    lambda: reply_check.check_inputs("request", {}),
])
def test_empty_list_accepts_and_single_diagnostic_is_unchanged(monkeypatch, call):
    validator(monkeypatch, [])
    assert call() is None
    validator(monkeypatch, ["single\n非 ASCII"])
    with pytest.raises(ValueError) as caught:
        call()
    assert str(caught.value) == "single\n非 ASCII"


@pytest.mark.parametrize("rows", [["first", 3], [None], "first"])
@pytest.mark.parametrize("kind", ["reply", "maker", "request"])
def test_malformed_diagnostics_are_infrastructure(monkeypatch, rows, kind):
    validator(monkeypatch, rows)
    with pytest.raises(reply_check.ReplyCheckInfrastructureError, match="invalid output"):
        if kind == "reply":
            reply_check.check_reply("reviewer", {}, {})
        else:
            reply_check.check_inputs(kind, {})


@pytest.mark.parametrize("error", [subprocess.TimeoutExpired("fixture", 5),
                                  subprocess.CalledProcessError(1, "fixture"), OSError()])
def test_failed_validator_never_becomes_a_repair(monkeypatch, error):
    monkeypatch.setenv("TL3_BINARY", "/fixture/tl3")
    def fail(*args, **kwargs):
        raise error
    monkeypatch.setattr(subprocess, "run", fail)
    with pytest.raises(reply_check.ReplyCheckInfrastructureError):
        reply_check.check_reply("reviewer", {}, {})
    with pytest.raises(reply_check.ReplyCheckInfrastructureError):
        reply_check.check_inputs("maker", {})


@pytest.mark.parametrize("role", ["ingest", "audit"])
def test_retention_transport_keeps_every_diagnostic(monkeypatch, role):
    monkeypatch.setattr(reply_check, "run_check", lambda *a, **k: {
        "reply": {}, "violations": ["retain first", "retain second", "retain first"]})
    reply = {"kind": "brief"}
    with pytest.raises(reply_check.ReplyViolation) as caught:
        preserve_verbatim(role, {"issue": {"body": "```text\nx\n```"}}, reply)
    assert str(caught.value) == "retain first\nretain second"
    assert reply == {"kind": "brief"}


def test_transport_preserves_unicode_and_newlines(monkeypatch):
    monkeypatch.setenv("TL3_BINARY", "/fixture/tl3")
    monkeypatch.setattr(subprocess, "run", lambda *a, **k: SimpleNamespace(
        stdout=json.dumps({"violations": ["first\n界", "second é"]})))
    with pytest.raises(reply_check.ReplyViolation, match="first\n界\nsecond é"):
        reply_check.check_reply("reviewer", {}, {})
