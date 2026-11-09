"""Read-only exact-unit helper fixtures. Run inside admitted development compute."""
import errno
import json
import os
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace

import pytest

from tl3_harness import runtime


def request():
    return {'schema': 'tl3.job_unit_absence_input.v1',
            'run': {'backend': 'flux', 'instance': 'fv1-model/' + 'a' * 32,
                    'uri': 'local:///fixture', 'job': 'fABC'},
            'owner': {'owner': 'fixture-owner', 'correlation': 'fixture-correlation'},
            'request_digest': 'b' * 64, 'host': 'dl380g10', 'nonce': 'c' * 64, 'budget_seconds': 5}


@pytest.mark.parametrize('case', ['control-directory-job-absent', 'control-directory-job-present', 'unreadable',
                                'foreign-host', 'before', 'after', 'nonzero',
                                'control-missing', 'control-unreadable', 'control-file'])
def test_exact_job_unit_absence_is_read_only_and_guarded(tmp_path, monkeypatch, case):
    guard = tmp_path / 'systemctl-fixture'
    calls = tmp_path / 'guard-calls'
    guard.write_text(f'''#!/usr/bin/python3
import json, pathlib, sys
path=pathlib.Path({str(calls)!r})
rows=path.read_text().splitlines() if path.exists() else []
with path.open('a') as log: log.write(json.dumps(sys.argv[1:])+'\\n')
wrong=({case!r}=='before' and not rows) or ({case!r}=='after' and bool(rows))
print('wrong' if wrong else {'a' * 32!r})
sys.exit(1 if {case!r}=='nonzero' else 0)
''')
    guard.chmod(0o700)
    monkeypatch.setattr(runtime.os, 'uname', lambda: SimpleNamespace(nodename='foreign' if case == 'foreign-host' else 'dl380g10'))
    original_stat = os.stat
    observed = []
    expected = '/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/flux.slice/flux-jobs.slice/shell-fABC:sys.service'
    control = '/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/flux.slice'
    failed_control = case in ('control-missing', 'control-unreadable', 'control-file')

    def exact_stat(path, *args, **kwargs):
        if str(path).startswith('/sys/fs/cgroup'):
            assert str(path) in (control, expected)
            assert kwargs == {'follow_symlinks': False}
            assert len(calls.read_text().splitlines()) == 1, 'both stats must be between generation guards'
            if str(path) == expected:
                assert observed == [control], 'positive control must precede exact job observation'
            observed.append(str(path))
            if str(path) == control:
                if case == 'control-missing':
                    raise FileNotFoundError(errno.ENOENT, 'fixture control absent')
                if case == 'control-unreadable':
                    raise PermissionError(errno.EACCES, 'fixture control unreadable')
                return SimpleNamespace(st_mode=0o100600 if case == 'control-file' else 0o40700)
            if case == 'control-directory-job-present':
                return SimpleNamespace(st_mode=0o40700)
            if case == 'unreadable':
                raise PermissionError(errno.EACCES, 'fixture unreadable')
            raise FileNotFoundError(errno.ENOENT, 'fixture absent')
        return original_stat(path, *args, **kwargs)

    monkeypatch.setattr(runtime.os, 'stat', exact_stat)
    result = runtime.job_unit_absence(request(), systemctl=str(guard))
    assert result['nonce'] == 'c' * 64 and result['path'] == expected
    assert result['run'] == request()['run'] and result['owner'] == request()['owner']
    rows = [json.loads(line) for line in calls.read_text().splitlines()]
    assert rows == [['--user', 'show', 'flux-vanilla-fv1-model.service', '-p', 'InvocationID', '--value']] * 2
    assert observed == ([] if case in ('foreign-host', 'before', 'nonzero') else
                        [control] if failed_control else [control, expected])
    assert result['control']['path'] == control
    assert result['control']['observation']['kind'] == (
        'unknown' if case in ('foreign-host', 'before', 'nonzero', 'control-unreadable') else
        'absent' if case == 'control-missing' else 'not_directory' if case == 'control-file' else 'directory')
    assert result['observation']['kind'] == ('present' if case == 'control-directory-job-present' else
                                           'unknown' if case in ('unreadable', 'foreign-host', 'before', 'nonzero')
                                           or failed_control else 'absent')
    assert result['generation_after']['stdout'].strip() == ('wrong' if case == 'after' else 'a' * 32)
    if destination := os.environ.get('FX_HELPER_EVIDENCE'):
        destination = Path(destination) / case
        destination.mkdir(parents=True, exist_ok=True)
        (destination / 'result.json').write_text(json.dumps(result))
        (destination / 'guard-calls.jsonl').write_bytes(calls.read_bytes())
        (destination / 'stat-order.json').write_text(json.dumps(observed))


@pytest.mark.parametrize('field,value', [('job', 'ƒABC'), ('job', '../foreign'), ('job', 'fABC/foreign'),
                                         ('instance', 'fv1-model/not-a-generation'), ('instance', '../' + 'a' * 32)])
def test_job_unit_mode_rejects_unqualified_identity_before_any_process_or_stat(monkeypatch, field, value):
    facts = request()
    facts['run'][field] = value
    monkeypatch.setattr(runtime.os, 'stat', lambda *args, **kwargs: pytest.fail('stat before identity validation'))
    with pytest.raises(ValueError, match='unqualified'):
        runtime.job_unit_absence(facts, systemctl='/must-not-execute')


def test_no_process_handles_are_never_cessation_evidence():
    assert runtime.processes_stopped([], 0) is False


def test_staged_cleanup_mode_is_bound_to_the_imported_source_hash(tmp_path):
    _, cleanup_manifest, package = runtime.stage_host_runtime(
        tmp_path / 'fixture-bundle', interpreter=sys.executable, producer_commit='d' * 40,
        podman='/__must_not_execute_podman__', systemctl='/__must_not_execute_systemctl__')
    import hashlib

    inventory = json.loads(cleanup_manifest.read_bytes())
    module = package / 'runtime.py'
    row = next(row for row in inventory['files'] if row['path'] == 'lib/tl3_harness/runtime.py')
    assert row['sha256'] == hashlib.sha256(module.read_bytes()).hexdigest()
    assert b'def job_unit_absence(' in module.read_bytes()
    assert b'def scratch_cleanup(' in module.read_bytes()
    assert b'tl3.scratch_cleanup_input.v1' in module.read_bytes()
    cleanup = cleanup_manifest.parent / 'host-cleanup'
    if destination := os.environ.get('FX_HELPER_EVIDENCE'):
        destination = Path(destination) / 'staged-pin'
        destination.mkdir(parents=True, exist_ok=True)
        (destination / 'inventory.json').write_bytes(cleanup_manifest.read_bytes())
        (destination / 'runtime.py').write_bytes(module.read_bytes())
        (destination / 'host-cleanup').write_bytes(cleanup.read_bytes())
    # --help exits before any host/runtime observation. The verifier still runs.
    result = subprocess.run([sys.executable, '-I', str(cleanup), '--help'], capture_output=True, check=False)
    assert result.returncode == 0, result.stderr
    module.write_bytes(module.read_bytes() + b'\n# fixture tamper\n')
    result = subprocess.run([sys.executable, '-I', str(cleanup), '--help'], capture_output=True, check=False)
    assert result.returncode != 0 and b'trusted runtime imported bytes changed' in result.stderr
