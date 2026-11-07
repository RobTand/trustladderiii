"""Probe the model worker's restored policy path without a Rust action executable."""
import json
import os
from pathlib import Path
import shutil
import sys
from types import SimpleNamespace

def main():
    trusted = Path(sys.argv[1]).resolve()
    assert all(key not in os.environ for key in (
        'TL3_ACTION_BINARY', 'RUNFILES_DIR', 'TEST_SRCDIR', 'RUNFILES_MANIFEST_FILE', 'PYTHONPATH'))
    assert shutil.which('tl3-action') is None
    assert not list(trusted.rglob('tl3-action'))
    sys.path.insert(0, str(trusted))
    from tl3_workers import contracts, evidence, judges, runtime, tools, tracing, turns
    from tl3_workers.rules import permits, render_all
    from jsonschema import Draft202012Validator

    assert runtime is not None
    secret = 'TL3FAKE' + ''.join(chr(65 + (i * 7) % 26) for i in range(24))
    redactor = tracing.Redactor(None)
    assert secret not in json.dumps(redactor.payload({'password': secret, 'note': 'ordinary'}))
    assert tracing.sensitive_path({'path': '.env'})
    value = {'note': 'ordinary', 'messages': ['small']}
    content = tracing.Content('mlflow.spanInputs', redactor.shape(value))
    root = tracing.Entry(SimpleNamespace(ByteSize=lambda: 64), 'root', 'CHAIN', [content])
    capture = tracing.Capture.__new__(tracing.Capture)
    capture.allocate(root, [root])
    assert 0 < content.limit < 512
    assert redactor.clean(content.value, content.limit) == value
    assert tracing.chunk_counts({'prompt_tokens': 3, 'total_tokens': True, 'unknown': 4}) == {'prompt_tokens': 3}

    violations = evidence.contract_violations({'tool_capabilities': []}, {})
    assert violations and '/evidence_contract' in render_all(violations, role='ingest')
    assert permits(violations, {}, {'evidence_contract': []}, role='ingest')
    validator = Draft202012Validator({'properties': {'count': {'type': 'integer'}}})
    violations = contracts.schema_violations(validator.iter_errors({'count': 'wrong'}), role='schema')
    assert '/count' in render_all(violations, role='schema')
    assert permits(violations, {'count': 'wrong'}, {'count': 1}, role='schema')
    refusal = tools.ChangeRefusal('protected', 'app.py', 'edit', ['app.py'])
    assert 'Cannot edit protected' in str(refusal) and refusal.violations
    assert len(turns.repair_message(ValueError('x' * 10000)).encode()) <= turns.REPAIR_MESSAGE_BYTES

    from mlflow.genai.judges.adapters import gateway_adapter
    observation = SimpleNamespace(request={'payload': {'messages': []}}, reply='invalid')
    sent = []
    def send_chat_request(**request):
        sent.append(request)
        observation.reply = json.dumps({'result': True, 'rationale': 'ordinary'})
    gateway_adapter.send_chat_request = send_chat_request
    feedback = judges.repair_grade(observation, 'job probe')
    assert feedback.value is True
    assert sent[0]['payload']['tool_choice'] == 'none'
    assert 'Tools are unavailable' in sent[0]['payload']['messages'][-1]['content']
    print(json.dumps({'entrypoints': ['runtime import', 'Redactor.payload', 'sensitive_path',
        'Capture.allocate', 'Redactor.clean', 'chunk_counts', 'contract_violations', 'permits',
        'schema_violations', 'ChangeRefusal', 'repair_message', 'repair_grade']}))


if __name__ == "__main__":
    main()
