"""The Python shim transports JSON; Rust owns classification decisions."""
import re
import sys

import pytest

from tl3_workers.roles.classifier import check_reply


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
