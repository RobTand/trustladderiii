import json

import pytest
import tiktoken

import test_workers
from test_workers import BRIEF
from worker_process import start_worker

request_doc = test_workers.request_doc


@pytest.fixture(scope="session", autouse=True)
def sizing_tokenizer(tmp_path_factory):
    with pytest.MonkeyPatch.context() as patch:
        patch.setenv("TIKTOKEN_CACHE_DIR", str(tmp_path_factory.mktemp("sizing-tokenizer")))
        from tiktoken.load import read_file_cached
        from tl3_workers.forecast import TOKENIZER_URL, TOKENIZER_SHA256
        read_file_cached(TOKENIZER_URL, TOKENIZER_SHA256)
        yield


def test_root_forecast_uses_declared_scope_and_shared_measurement(tmp_path, request_doc):
    (tmp_path / "greeting.py").write_text("hello world\n")
    (tmp_path / "unrelated.py").write_text("unrelated\n" * 1000)
    request_doc["inputs"]["repo"]["file_list"].append("unrelated.py")
    request_doc["inputs"]["forecast"] = {
        "fit_tokens": 100, "context_tokens": 150,
        "overhead_tokens": {"prompt": 10, "making": 20},
    }
    reply = {**BRIEF, "files": ["greeting.py", "new.py"]}
    result = start_worker(tmp_path, request_doc, [{"content": json.dumps(reply)}])
    assert result.exit_code == 0, result.stderr
    forecast = result.events[-1]["reply"]["forecast"]
    encoding = tiktoken.get_encoding("o200k_base")
    assert forecast["tokens"] == len(encoding.encode_ordinary(BRIEF["brief"])) + 3 + 30
    assert forecast["file_tokens"] == {"greeting.py": 3, "new.py": 0}


FORGED = {**BRIEF, "files": [], "forecast": {
    "tokenizer": "o200k_base", "brief_tokens": 1, "file_tokens": {},
    "overhead_tokens": 10, "tokens": 11, "fit_tokens": 100,
    "context_tokens": 150, "fits": True,
}}


@pytest.mark.parametrize("reply", [BRIEF, FORGED])
def test_sized_ingest_requires_declarations_and_refuses_forged_evidence(tmp_path, request_doc, reply):
    from tl3_workers.contracts import validate
    validate("ingest-reply", reply)
    request_doc["inputs"]["forecast"] = {
        "fit_tokens": 100, "context_tokens": 150, "overhead_tokens": {"prompt": 10},
    }
    result = start_worker(tmp_path, request_doc, [{"content": json.dumps(reply)}])
    assert result.exit_code == 1
    assert not any(event["event"] == "reply" for event in result.events)
