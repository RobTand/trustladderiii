"""The Python shim transports JSON; Rust owns classification decisions."""
import json
import re
import sys

import pytest

from tl3_workers.roles.classifier import check_reply
from tl3_workers import reply_check
from tl3_workers.tracing import failure_tags
from tl3_workers.turns import InvalidReply, TurnControl, repaired


@pytest.fixture
def fake_tl3(tmp_path, monkeypatch):
    def install(body):
        checker = tmp_path / "fake-tl3"
        checker.write_text(f"#!{sys.executable}\n" + body)
        checker.chmod(0o755)
        monkeypatch.setenv("TL3_BINARY", str(checker))
        return checker
    return install


@pytest.mark.parametrize("violations", [[], ["canned repair message: café"]])
def test_classifier_shim_transports_the_subprocess_contract(tmp_path, monkeypatch, violations):
    request = {"inputs": {"comment": "untrusted café"}, "reply": {"kind": "remark"}}
    checker = tmp_path / "fake-tl3"
    marker = tmp_path / "called.json"
    response = {"violations": violations}
    checker.write_text(
        f"#!{sys.executable}\n"
        "import json, sys\n"
        "from pathlib import Path\n"
        "assert sys.argv[1:] == ['check-reply', 'classifier']\n"
        f"assert json.load(sys.stdin) == {request!r}\n"
        f"Path({str(marker)!r}).write_text('called')\n"
        f"print(json.dumps({response!r}))\n"
    )
    checker.chmod(0o755)
    monkeypatch.setenv("TL3_BINARY", str(checker))
    if violations:
        with pytest.raises(ValueError, match=re.escape(violations[0])):
            check_reply(**request)
    else:
        assert check_reply(**request) is None
    assert marker.read_text() == "called"


@pytest.mark.parametrize("response", [
    *({"violations": value} for value in [
        None, False, 0, "", {}, "repair", [None], [False], [0], [{}], [[]],
        ["restart is not offered for this request", None],
    ]),
    None, False, 0, "", [], {},
])
def test_classifier_shim_rejects_malformed_validator_output(fake_tl3, response):
    fake_tl3(f"print({json.dumps(response)!r})\n")
    with pytest.raises(Exception) as caught:
        check_reply({}, {"kind": "remark"})
    assert type(caught.value).__name__ == "ReplyCheckInfrastructureError"
    assert failure_tags(caught.value) == {
        "failure_class": "ReplyCheckInfrastructureError",
        "failure_message": "reply validator returned invalid output",
    }


@pytest.mark.parametrize("failure,message", [
    ("hang", "reply validator timed out"),
    ("crash", "reply validator process failed"),
    ("missing", "reply validator could not start"),
    ("garbage", "reply validator returned invalid output"),
])
def test_classifier_shim_infrastructure_failure_never_enters_repair(
        fake_tl3, monkeypatch, failure, message):
    checker = fake_tl3({
        "hang": "import time\ntime.sleep(0.3)\nprint('{\"violations\": []}')\n",
        "crash": "import sys\nprint('private-validator-canary', file=sys.stderr)\nsys.exit(7)\n",
        "missing": "",
        "garbage": "print('private-validator-canary')\n",
    }[failure])
    if failure == "missing":
        checker.unlink()
    # The fixture outlives this deadline but still terminates on the unfixed shim.
    if failure == "hang":
        monkeypatch.setattr(reply_check, "TIMEOUT_SECONDS", 0.05, raising=False)
    asked = []
    history = []

    def ask():
        asked.append(True)
        return '{"kind":"remark"}'

    with pytest.raises(Exception) as caught:
        repaired(TurnControl(), history, ask, lambda _: check_reply({}, {"kind": "remark"}))
    assert type(caught.value).__name__ == "ReplyCheckInfrastructureError"
    assert asked == [True]
    assert history == []
    assert failure_tags(caught.value) == {
        "failure_class": "ReplyCheckInfrastructureError", "failure_message": message,
    }
    assert "private-validator-canary" not in str(caught.value)


def test_classifier_shim_keeps_exact_repair_and_failure_diagnostics(fake_tl3):
    first = "restart is not offered for this request"
    second = "a change requires new request text"
    fake_tl3(f"print({json.dumps({'violations': [first, second]})!r})\n")
    history = []
    with pytest.raises(InvalidReply) as caught:
        repaired(TurnControl(), history, lambda: '{"kind":"restart"}',
                 lambda _: check_reply({}, {"kind": "restart"}))
    assert all(type(error) is not ValueError for error in caught.value.errors)
    assert all(str(error) == first for error in caught.value.errors)
    assert history[0].text == (
        f"Your reply failed validation: {first}\nTools are unavailable for this turn. Return one "
        "corrected JSON object matching your reply schema and nothing else.")
    assert failure_tags(caught.value.errors[0]) == {
        "failure_class": "ReplyViolation", "failure_message": first,
    }
    assert failure_tags(caught.value) == {
        "failure_class": "InvalidReply",
        "failure_message": f"invalid reply: {first}; repaired reply: {first}",
    }


@pytest.mark.parametrize("response", [
    '{"violations":["reject"],"violations":[]}',
    '{"violations":[],"violations":["reject"]}',
    '{"violations":[],"metadata":{"private-validator-canary":1,"private-validator-canary":2}}',
    '{"violations":[],"metadata":[{"inner":{"duplicate":1,"duplicate":2}}]}',
])
def test_reply_shim_duplicate_keys_are_infrastructure_failure(fake_tl3, response):
    fake_tl3(f"print({response!r})\n")
    asked = []
    history = []

    def ask():
        asked.append(True)
        return '{"kind":"remark"}'

    with pytest.raises(reply_check.ReplyCheckInfrastructureError) as caught:
        repaired(TurnControl(), history, ask, lambda _: check_reply({}, {"kind": "remark"}))
    assert asked == [True]
    assert history == []
    assert failure_tags(caught.value) == {
        "failure_class": "ReplyCheckInfrastructureError",
        "failure_message": "reply validator returned invalid output",
    }
    assert "private-validator-canary" not in str(caught.value)
