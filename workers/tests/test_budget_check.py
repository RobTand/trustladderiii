"""Transport failure is infrastructure and never a successful budget decision."""
import json
import subprocess
from types import SimpleNamespace

import pytest
from tl3_workers import budget_check
from tl3_workers import reply_check
from tl3_workers.reply_check import ReplyCheckInfrastructureError


@pytest.mark.parametrize("stdout", [
    '{"result": null}', '{"result": 1}', '{"result": "false"}',
    '{"result": true, "extra": 1}', '{"result": true, "result": false}',
    '[]', '{}', 'false', 'not JSON',
])
def test_malformed_output_never_accepts(monkeypatch, stdout):
    monkeypatch.setenv("TL3_BINARY", "/fixture/tl3")
    monkeypatch.setattr(subprocess, "run", lambda *args, **kwargs: SimpleNamespace(stdout=stdout))
    with pytest.raises(ReplyCheckInfrastructureError, match="invalid output"):
        budget_check.check("compaction", before=10, after=2)


def test_timeout_and_process_failure_bypass_model_repair(monkeypatch):
    monkeypatch.setenv("TL3_BINARY", "/fixture/tl3")
    for error in [subprocess.TimeoutExpired("tl3", 5), subprocess.CalledProcessError(1, "tl3"), OSError()]:
        def fail(*args, **kwargs):
            raise error
        monkeypatch.setattr(subprocess, "run", fail)
        with pytest.raises(ReplyCheckInfrastructureError):
            budget_check.check("compaction", before=10, after=2)


def test_transport_forwards_exact_inputs_and_uses_the_release_binary(monkeypatch):
    monkeypatch.setenv("TL3_BINARY", "/fixture/tl3")
    def run(argv, **kwargs):
        assert argv == ["/fixture/tl3", "check-budget"]
        assert json.loads(kwargs["input"]) == {"kind": "compaction", "before": 10, "after": 2}
        assert kwargs["timeout"] == 5 and kwargs["check"] is True
        return SimpleNamespace(stdout='{"result": true}')
    monkeypatch.setattr(subprocess, "run", run)
    assert budget_check.check("compaction", before=10, after=2) is True


@pytest.mark.parametrize("constant", ["NaN", "Infinity", "-Infinity"])
def test_non_json_constants_fail_even_in_open_object_contracts(monkeypatch, constant):
    monkeypatch.setenv("TL3_BINARY", "/fixture/tl3")
    monkeypatch.setattr(subprocess, "run", lambda *args, **kwargs: SimpleNamespace(
        stdout='{"result": {"unexpected": ' + constant + '}}'))
    with pytest.raises(ReplyCheckInfrastructureError, match="invalid output"):
        budget_check.check("restore", state={}, maker=True)


def test_budget_uses_the_shared_reply_validator_transport(monkeypatch):
    """#780/#781/#782: both commands share strict parsing and process failures."""
    observed = []

    def shared(command, document, *, name="reply"):
        observed.append((command, document, name))
        return {"result": True}

    def separate(*args, **kwargs):
        pytest.fail("budget decision bypassed the shared validator transport")

    monkeypatch.setenv("TL3_BINARY", "/fixture/tl3")
    monkeypatch.setattr(reply_check, "run_check", shared, raising=False)
    monkeypatch.setattr(subprocess, "run", separate)
    assert budget_check.check("compaction", before=10, after=2) is True
    assert observed == [(["check-budget"], {"kind": "compaction", "before": 10, "after": 2}, "budget")]
