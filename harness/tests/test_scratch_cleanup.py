"""Exact-root fixtures. These temporary trees and processes run only inside PB."""
import json
import os
import uuid
from pathlib import Path
from types import SimpleNamespace

import pytest

from tl3_harness import runtime


def authorize(output, request, approved):
    import hashlib

    raw = json.dumps({'scratch_root': approved}).encode()
    request['request_digest'] = hashlib.sha256(raw).hexdigest()
    (output / 'request.json').write_bytes(raw)
    intent = {'backend': 'flux', 'instance': request['run']['instance'], 'uri': request['run']['uri'],
              **request['owner'], 'request_digest': request['request_digest'], 'output_dir': str(output)}
    (output / 'intent.json').write_text(json.dumps(intent))


def observe(request, output):
    (output / ('scratch-absence-' + request['nonce'] + '.input.json')).write_text(json.dumps(request))
    return runtime.scratch_cleanup(request, systemctl='/never-called')


@pytest.fixture
def scratch(tmp_path, monkeypatch):
    correlation = 'scratch-fixture-' + uuid.uuid4().hex
    output = Path('/mnt/shared/flux-out/tl3') / correlation
    output.mkdir(mode=0o700)
    approved = tmp_path / 'scratch'
    approved.mkdir(mode=0o700)
    root = approved / 'fABC'
    root.mkdir(mode=0o700)
    (root / 'payload').mkdir()
    (root / 'payload/file').write_text('owned fixture bytes')
    request = {'schema': 'tl3.scratch_cleanup_input.v1',
               'run': {'backend': 'flux', 'instance': 'fv1-model/' + 'a' * 32,
                       'uri': 'local:///fixture', 'job': 'fABC'},
               'owner': {'owner': 'fixture-owner', 'correlation': correlation},
               'request_digest': 'b' * 64, 'host': 'dl380g10', 'nonce': 'c' * 64,
               'budget_seconds': 5,
               'reclamation': 'external scratch reclamation'}
    authorize(output, request, str(approved))
    guards = []

    def guard(systemctl, instance, deadline):
        guards.append(True)
        return {'exit_code': 0, 'stdout': 'a' * 32 + '\n', 'stderr': ''}

    monkeypatch.setattr(runtime, '_external_generation', guard)
    monkeypatch.setattr(runtime.os, 'uname', lambda: SimpleNamespace(nodename='dl380g10'))
    yield request, root, output, guards
    import shutil
    shutil.rmtree(output)


def retain(case, result, output):
    if destination := os.environ.get('FX_SCRATCH_EVIDENCE'):
        destination = Path(destination) / case
        destination.mkdir(parents=True, exist_ok=True)
        (destination / 'observation.json').write_text(json.dumps(result))
        for path in output.iterdir():
            (destination / path.name).write_bytes(path.read_bytes())


def test_guarded_reclamation_retains_manifest_before_first_mutation(scratch, monkeypatch):
    request, root, output, guards = scratch
    unlink = runtime.os.unlink
    mutations = []

    def guarded(name, *, dir_fd=None):
        manifest = json.loads((output / 'scratch-reclamation.manifest.json').read_text())
        assert manifest['path'] == str(root) and len(manifest['entries']) == 2
        assert len(guards) == 2
        mutations.append(name)
        return unlink(name, dir_fd=dir_fd)

    monkeypatch.setattr(runtime.os, 'unlink', guarded)
    result = observe(request, output)
    monkeypatch.setattr(runtime.os, 'unlink', unlink)
    assert result['observation'] == {'kind': 'absent', 'errno': 2}
    assert result['attribution'] == 'external scratch reclamation' and mutations == ['file']
    assert not root.exists() and len(guards) == 3
    assert json.loads((output / 'scratch-reclamation.complete.json').read_text()) == result
    retain('reclaimed', result, output)


def test_wrong_executing_uid_rejects_before_scratch_authority(scratch, monkeypatch):
    request, root, output, guards = scratch
    with monkeypatch.context() as wrong_account:
        wrong_account.setattr(runtime.os, 'getuid', lambda: 1001)
        wrong_account.setattr(runtime.os, 'open', lambda *args, **kwargs: pytest.fail('open before uid guard'))
        wrong_account.setattr(runtime.os, 'stat', lambda *args, **kwargs: pytest.fail('stat before uid guard'))
        wrong_account.setattr(runtime, '_external_generation', lambda *args: pytest.fail('generation before uid guard'))
        for entry in (lambda: runtime.scratch_cleanup(request, systemctl='/must-not-execute'),
                      lambda: runtime._scratch_cleanup_bound(request, request['run']['instance'].split('/'),
                                                             root.parent, systemctl='/must-not-execute')):
            with pytest.raises(ValueError, match='executing uid1000'):
                entry()
    assert root.joinpath('payload/file').read_text() == 'owned fixture bytes'
    assert not (output / 'scratch-reclamation.manifest.json').exists()
    assert not (output / 'scratch-reclamation.complete.json').exists()
    assert guards == []
    retain('wrong-executing-uid', {'rejected_before_open_stat_generation': True,
                                 'scratch_untouched': True, 'manifest_absent': True}, output)


@pytest.mark.parametrize('case', ['ordinary', 'symlink-root', 'symlink-child', 'wrong-owner',
                                 'cross-filesystem', 'unreadable', 'failed-removal', 'over-bound', 'over-bytes',
                                 'foreign-host'])
def test_guards_never_certify_failed_or_unsafe_removal(scratch, monkeypatch, case):
    request, root, output, guards = scratch
    original_stat = runtime.os.stat
    if case == 'ordinary':
        request['reclamation'] = None
    elif case == 'symlink-root':
        root.rename(root.with_name('retained'))
        root.symlink_to(root.with_name('retained'), target_is_directory=True)
    elif case == 'symlink-child':
        (root / 'link').symlink_to(root / 'payload/file')
    elif case in ('wrong-owner', 'cross-filesystem', 'over-bytes'):
        def changed(path, *args, **kwargs):
            info = original_stat(path, *args, **kwargs)
            if path == 'file' and kwargs.get('dir_fd') is not None:
                fields = list(info)
                if case == 'over-bytes':
                    # One entry over the 2 GiB apparent-size bound, without allocating it.
                    fields[6] = 2 * 1024 ** 3 + 1
                else:
                    fields[4 if case == 'wrong-owner' else 2] += 1
                return os.stat_result(fields)
            return info
        monkeypatch.setattr(runtime.os, 'stat', changed)
    elif case == 'unreadable':
        monkeypatch.setattr(runtime.os, 'scandir', lambda *a, **k: (_ for _ in ()).throw(PermissionError('fixture unreadable')))
    elif case == 'failed-removal':
        def refused(*args, **kwargs):
            raise PermissionError('fixture removal refused')
        monkeypatch.setattr(runtime.os, 'unlink', refused)
    elif case == 'over-bound':
        for index in range(4097):
            (root / f'entry-{index}').touch()
    elif case == 'foreign-host':
        monkeypatch.setattr(runtime.os, 'uname', lambda: SimpleNamespace(nodename='foreign'))
    result = observe(request, output)
    monkeypatch.undo()
    assert result['observation']['kind'] == ('present' if case == 'ordinary' else 'unknown')
    assert result['attribution'] is None
    assert root.exists() and not (output / 'scratch-reclamation.complete.json').exists()
    if case != 'failed-removal':
        assert not (output / 'scratch-reclamation.manifest.json').exists()
    retain(case, result, output)


@pytest.mark.parametrize('path', ['/mnt/shared/unsafe', '/tmp/../foreign', 'relative', '/'])
def test_noncanonical_approved_root_rejected_before_generation(scratch, monkeypatch, path):
    request, root, output, guards = scratch
    authorize(output, request, path)
    with pytest.raises(ValueError, match='unqualified'):
        observe(request, output)
    assert not guards and root.exists()


def test_failed_reclamation_is_not_retried_even_if_root_disappears(scratch, monkeypatch):
    request, root, output, guards = scratch
    unlink = runtime.os.unlink
    monkeypatch.setattr(runtime.os, 'unlink', lambda *a, **k: (_ for _ in ()).throw(PermissionError('fixture')))
    first = observe(request, output)
    monkeypatch.setattr(runtime.os, 'unlink', unlink)
    assert first['observation']['kind'] == 'unknown'
    import shutil
    shutil.rmtree(root)
    request['nonce'] = 'd' * 64
    second = observe(request, output)
    assert second['observation']['kind'] == 'unknown'
    assert 'no retry' in second['observation']['reason']


def test_scoped_action_handler_suppresses_repeat_term_and_restores_previous(tmp_path, monkeypatch):
    import signal

    monkeypatch.setattr(runtime, 'launch_context', lambda: {'output_dir': str(tmp_path)})
    previous = signal.getsignal(signal.SIGTERM)
    with pytest.raises(SystemExit) as ended:
        with runtime.cancellation_unwind():
            handler = signal.getsignal(signal.SIGTERM)
            try:
                handler(signal.SIGTERM, None)
            finally:
                assert signal.getsignal(signal.SIGTERM) == signal.SIG_IGN
    assert ended.value.code == 143 and signal.getsignal(signal.SIGTERM) == previous
    assert json.loads((tmp_path / 'action-sigterm.json').read_text())['signal'] == 15


def test_caller_cannot_supply_a_deletion_root_or_change_retained_binding(scratch):
    request, root, output, guards = scratch
    request['approved_root'] = '/tmp/foreign'
    with pytest.raises(ValueError, match='fields'):
        runtime.scratch_cleanup(request, systemctl='/never-called')
    request.pop('approved_root')
    observe_input = output / ('scratch-absence-' + request['nonce'] + '.input.json')
    observe_input.write_text(json.dumps(request))
    request['run'] = {**request['run'], 'job': 'fDEF'}
    with pytest.raises(ValueError, match='retained Rust authority'):
        runtime.scratch_cleanup(request, systemctl='/never-called')
    assert not guards and root.exists()


def test_broker_metadata_changes_do_not_change_scratch_ownership(scratch, monkeypatch):
    request, root, output, guards = scratch
    monkeypatch.setattr(runtime, '_external_generation', lambda *args: {
        'exit_code': 0, 'stdout': 'after-reboot', 'stderr': ''})
    result = observe(request, output)
    assert result['observation']['kind'] == 'absent'
    assert result['attribution'] == request['reclamation']
    assert not root.exists()
