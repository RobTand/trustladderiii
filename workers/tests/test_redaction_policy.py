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
@pytest.mark.parametrize('launch', ['import', 'script'])
def test_worker_import_refuses_invalid_policy(tmp_path, field, value, matching_hash, launch):
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
    command = ([sys.executable, '-I', str(package / 'tracing.py')] if launch == 'script' else
               [sys.executable, '-I', '-c',
                'import sys; sys.path.insert(0,sys.argv[1]); import tl3_workers; print("worker-started")',
                str(tmp_path)])
    result = subprocess.run(command, capture_output=True, text=True, check=False)
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


def test_detached_exporter_loads_the_bound_policy():
    """#753: script startup must validate the same sidecar as worker package import."""
    script = Path(tl3_workers.__file__).with_name('tracing.py')
    result = subprocess.run([sys.executable, '-I', str(script)],
                            capture_output=True, text=True, check=False)
    assert result.returncode == 2
    assert 'usage: tracing.py' in result.stderr
    assert 'redaction policy' not in result.stderr.lower()


def test_shared_redactor_matches_pre_extraction_reference():
    """#826: moving the matcher keeps its policy reads and observable text unchanged."""
    from tl3_workers import tracing
    source = Path(__file__).with_name('fixtures') / 'stderr-redactor-before.txt'
    reference = {'__name__': 'stderr_reference', '__file__': tracing.__file__}
    exec(compile(source.read_text(), str(source), 'exec'), reference)
    from redaction_branches import branch_inputs
    from redaction_original_inputs import original_inputs
    inputs = original_inputs() + branch_inputs() + [
        'password: zE9!pR4#cV7$ ' + '.' * 499,
        'password: zE9!pR4#cV7$\n' + '.' * 505 + '\n' + '.' * 504,
        'password: ' + 'v' * 20000 + '\nordinary cause\n',
    ]
    before, after = reference['Redactor'](None), tracing.Redactor(None)
    for text in inputs:
        assert before.text(text, limit=len(text)) == after.text(text, limit=len(text))


def test_harness_policy_matches_the_verified_worker_policy():
    """#826: a staged pytest harness cannot silently use a different Rust policy."""
    from tl3_harness import redaction
    from tl3_workers import tracing
    assert redaction.POLICY == tracing.load_policy()


def test_contextual_azure_sas_is_secret_in_real_job_text_windows_and_payloads():
    from tl3_harness.redaction import Redactor
    import tracing_forms3

    _, secrets = next((text, secrets) for name, text, secrets, _ in tracing_forms3.FORMS
                           if name == 'presigned_azure_sas_sig')
    signature = secrets[0]
    for query in [
        'sv=2022-11-02&se=2026-12-01&sr=b&sig=' + signature,
        'sig=' + signature + '&ss=b&se=2026-12-01&sv=2022-11-02',
        '%73v=2022-11-02&%73e=2026-12-01&%73r=b&%73ig=' + signature,
    ]:
        source = 'https://st42.blob.core.windows.net/c/b?' + query + '&after=ordinary'
        expected = source.replace(signature, '[REDACTED]')
        redactor = Redactor(None)
        matches = redactor.text(source) == expected
        assert matches, "SAS text/context mismatch"
        start = source.index(signature) + 3
        matches = redactor.window(source, start, start + 16) == '[REDACTED]'
        assert matches, 'SAS window mismatch'
        matches = redactor.payload({'note': source}) == {'note': expected}
        assert matches, 'SAS payload mismatch'
        for offset in range(len(signature) - 8 + 1):
            survives = signature[offset:offset + 8] in expected
            assert not survives, f"SAS fragment offset {offset}"


def test_generic_or_incomplete_urls_do_not_acquire_azure_sas_semantics():
    from tl3_harness.redaction import azure_sas_spans

    signature = 'TL3FAKE' + 'a' * 36 + '%2B%2F%3D'
    sources = [
        'ordinary sig=' + signature,
        'https://example.com/c?sv=v&se=e&sr=b&sig=' + signature,
        'https://st42.blob.core.windows.net.evil.example/c?sv=v&se=e&sr=b&sig=' + signature,
        'https://st42.blob.core.windows.net/c?sv=v&sr=b&sig=' + signature,
        'https://st42.blob.core.windows.net/c?sv=v&se=&sr=b&sig=' + signature,
        'https://st42.blob.core.windows.net/c#?sv=v&se=e&sr=b&sig=' + signature,
        'https://st42.blob.core.windows.net@evil.example/c?sv=v&se=e&sr=b&sig=' + signature,
        'https://@st42.blob.core.windows.net/c?sv=v&se=e&sr=b&sig=' + signature,
        'https://st42.blob.core.windows.net/c?sv=v&%73v=w&se=e&sr=b&sig=' + signature,
        'https://st42.blob.core.windows.net/c?sv=%GG&se=e&sr=b&sig=' + signature,
    ]
    for source in sources:
        assert list(azure_sas_spans(source)) == []
    # The policy bounds raw UTF8 bytes, including unrelated query pairs.
    for value in ['x' * 4097, 'é' * 2049]:
        source = 'https://st42.blob.core.windows.net/c?sv=v&se=e&sr=b&sig=' + signature + '&extra=' + value
        assert list(azure_sas_spans(source)) == []
