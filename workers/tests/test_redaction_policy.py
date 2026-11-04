"""#753: a worker must reject an incomplete or mismatched policy before startup."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys

import pytest
import tl3_workers


@pytest.mark.parametrize('field,value', [
    ('patterns', []), ('checked', []), ('patterns', ['']),
    ('patterns', 'not-a-list'), ('checked', [['x', 'unknown-check']]),
    ('regex', {}), ('words', {}), ('characters', {}),
    ('context', True), ('near_chars', -1), ('schema_version', 753),
    ('patterns:0', ''), ('patterns:0', 753), ('checked:0', ['x', 'unknown-check']),
    ('checked:0', [753, 'check_labelled']), ('regex:BEARER', 753),
    ('words:SECRET_WORDS', 'not-a-list'), ('words:SECRET_WORDS', []),
    ('characters:alnum', []), ('helpers:HEADER_REFERENCE', ''),
    ('regex:BEARER', '('),
])
@pytest.mark.parametrize('matching_hash', [False, True])
def test_worker_import_refuses_invalid_policy(tmp_path, field, value, matching_hash):
    source = Path(tl3_workers.__file__).parent
    package = tmp_path / 'tl3_workers'
    package.mkdir()
    for name in ('__init__.py', 'tracing.py', '_policy_identity.py'):
        if (source / name).exists():
            shutil.copyfile(source / name, package / name)
    original = (source / 'policy.json').read_bytes()
    policy = json.loads(original)
    if ':' in field:
        field, key = field.split(':')
        policy[field][int(key) if key.isdecimal() else key] = value
    else:
        policy[field] = value
    changed = json.dumps(policy).encode()
    (package / 'policy.json').write_bytes(changed)
    identity = package / '_policy_identity.py'
    if matching_hash and identity.exists():
        old, new = hashlib.sha256(original).hexdigest(), hashlib.sha256(changed).hexdigest()
        text = identity.read_text()
        assert old in text
        identity.write_text(text.replace(old, new))
    result = subprocess.run(
        [sys.executable, '-I', '-c',
         'import sys; sys.path.insert(0,sys.argv[1]); import tl3_workers; print("worker-started")',
         str(tmp_path)], capture_output=True, text=True, check=False)
    assert result.returncode != 0, 'invalid policy allowed worker import'
    assert 'worker-started' not in result.stdout
    assert 'redaction policy' in result.stderr.lower()


def test_worker_import_refuses_stale_matching_structure(tmp_path):
    source = Path(tl3_workers.__file__).parent
    package = tmp_path / 'tl3_workers'
    package.mkdir()
    for name in ('__init__.py', 'tracing.py', '_policy_identity.py'):
        if (source / name).exists():
            shutil.copyfile(source / name, package / name)
    policy = json.loads((source / 'policy.json').read_bytes())
    policy['patterns'][0] += '(?:stale-build-marker)'
    (package / 'policy.json').write_text(json.dumps(policy))
    result = subprocess.run(
        [sys.executable, '-I', '-c',
         'import sys; sys.path.insert(0,sys.argv[1]); import tl3_workers', str(tmp_path)],
        capture_output=True, text=True, check=False)
    assert result.returncode != 0, 'stale policy allowed worker import'
    assert 'redaction policy' in result.stderr.lower()
