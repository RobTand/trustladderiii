"""Controlled store executables exercise streaming transfer without a daemon."""
import json
import os
from pathlib import Path
import subprocess
import sys

import pytest


@pytest.fixture
def store(tmp_path):
    tmp_path.chmod(0o700)
    executable = tmp_path / 'runtime'
    executable.write_text(f'''#!{sys.executable}
import json
import pathlib
import sys
root = pathlib.Path({str(tmp_path)!r})
args = sys.argv[1:]
with (root / 'calls').open('a') as calls:
    calls.write(json.dumps(args) + '\\n')
if any('{{{{json .Id}}}}' in argument for argument in args):
    sys.exit(125)
if args[0] == 'save':
    sys.stdout.buffer.write(b'controlled-archive')
elif args[:2] == ['pull', '--tls-verify=false']:
    (root / 'present').write_text('yes')
elif args == ['load']:
    assert sys.stdin.buffer.read() == b'controlled-archive'
    (root / 'present').write_text('yes')
elif args[:2] == ['image', 'exists']:
    sys.exit(0 if (root / 'present').exists() else 1)
elif args[:2] == ['image', 'inspect']:
    print((root / 'metadata').read_text() if (root / 'metadata').exists() else '{{"Id":"sha256:fixture","Os":"linux","Architecture":"amd64","RepoDigests":[]}}')
else:
    sys.exit(125)
''')
    executable.chmod(0o700)
    return tmp_path, str(executable)


def test_import_is_streamed_and_existing_identity_is_revalidated_without_export(store):
    from tl3_harness.runtime_image import ensure_image

    root, executable = store
    observed = []

    def transfer():
        return ensure_image('sha256:fixture', podman=executable, exporter=executable,
                            environment={'PATH': os.defpath, 'HOME': str(root), 'XDG_RUNTIME_DIR': str(root)}, log=root / 'import.log', timeout=10,
                            validate=observed.append)

    first = transfer()
    assert transfer() == first
    commands = [json.loads(line) for line in (root / 'calls').read_text().splitlines()]
    assert commands.count(['save', 'sha256:fixture']) == 1
    assert commands.count(['load']) == 1
    assert len([command for command in commands if command[:2] == ['image', 'inspect']]) == 2
    assert len(observed) == 2
    assert all('run' not in command and 'commit' not in command for command in commands)


def test_rust_preflight_rejection_is_not_silently_replaced_with_another_image(store):
    from tl3_harness.runtime_image import ensure_image

    root, executable = store
    (root / 'present').write_text('yes')

    def reject(metadata):
        assert json.loads(metadata)['Architecture'] == 'amd64'
        raise RuntimeError('Rust rejects expected architecture or identity')

    with pytest.raises(RuntimeError, match='Rust rejects'):
        ensure_image('sha256:fixture', podman=executable, exporter=executable,
                     environment={'PATH': os.defpath, 'HOME': str(root), 'XDG_RUNTIME_DIR': str(root)}, log=root / 'import.log', timeout=10,
                     validate=reject)
    commands = [json.loads(line) for line in (root / 'calls').read_text().splitlines()]
    assert all(command[0] != 'save' and command != ['load'] for command in commands)


def test_approved_registry_pull_is_pinned_and_explicitly_disables_tls_verification(store):
    from tl3_harness.runtime_image import ensure_image

    root, executable = store
    reference = 'dl380g10:5000/tl3/sandbox@sha256:' + 'a' * 64
    environment = {'PATH': os.defpath, 'HOME': str(root), 'XDG_RUNTIME_DIR': str(root)}
    for _ in range(2):
        ensure_image(reference, podman=executable, exporter=None, environment=environment,
                     log=root / 'import.log', timeout=10, validate=lambda metadata: None, registry=True)
    commands = [json.loads(line) for line in (root / 'calls').read_text().splitlines()]
    assert commands.count(['pull', '--tls-verify=false', reference]) == 1
    assert not any(command[0] == 'save' or command == ['load'] for command in commands)


def test_runtime_directory_failure_does_not_repair_or_start_a_runtime(store):
    from tl3_harness.runtime_image import ensure_image

    root, executable = store
    root.chmod(0o755)
    with pytest.raises(RuntimeError, match='mode 0700'):
        ensure_image('sha256:fixture', podman=executable, exporter=executable,
                     environment={'HOME': str(root), 'XDG_RUNTIME_DIR': str(root)},
                     log=root / 'import.log', timeout=10, validate=lambda metadata: None)
    assert root.stat().st_mode & 0o777 == 0o755
    assert not (root / 'calls').exists()


@pytest.fixture
def profile_map(tmp_path):
    from hashlib import sha256

    entry = {'profile': 'sandbox', 'registry_ref': 'dl380g10:5000/tl3/sandbox@sha256:' + 'a' * 64,
             'manifest_digest': 'sha256:' + 'a' * 64, 'config_digest': 'sha256:' + 'b' * 64,
             'platform': 'linux/amd64', 'rootless_present_hosts': ['dl380g10']}
    path = tmp_path / 'image-map.json'
    path.write_text(json.dumps({'profiles': [entry]}))
    return path, entry, sha256(path.read_bytes()).hexdigest()


def test_image_profile_is_resolved_from_unique_ops_map_with_provenance(profile_map):
    from tl3_harness.runtime_image import resolve_profile

    path, entry, digest = profile_map
    resolved = resolve_profile(path, 'sandbox', host='dl380g10', platform='linux/amd64')
    assert resolved['profile'] == entry
    assert resolved['map'] == {'path': str(path), 'sha256': digest, 'bytes': path.stat().st_size}
    assert path.read_text() == json.dumps({'profiles': [entry]})


@pytest.mark.parametrize('defect', ['duplicate', 'missing', 'manifest', 'config', 'host', 'platform'])
def test_image_map_refuses_ambiguous_or_unavailable_profile(profile_map, defect):
    from tl3_harness.runtime_image import resolve_profile

    path, entry, _ = profile_map
    profiles = [entry]
    if defect == 'duplicate':
        profiles.append(dict(entry))
    elif defect == 'missing':
        profiles.clear()
    elif defect == 'manifest':
        entry['manifest_digest'] = 'sha256:' + 'c' * 64
    elif defect == 'config':
        entry['config_digest'] = 'sandbox:latest'
    elif defect == 'host':
        entry['rootless_present_hosts'] = ['another-host']
    else:
        entry['platform'] = 'linux/arm64'
    path.write_text(json.dumps({'profiles': profiles}))
    with pytest.raises(ValueError):
        resolve_profile(path, 'sandbox', host='dl380g10', platform='linux/amd64')


def test_map_preflight_never_transfers_a_missing_image(store, profile_map):
    from tl3_harness.runtime_image import preflight_profile

    root, executable = store
    path, _, _ = profile_map
    with pytest.raises(subprocess.CalledProcessError):
        preflight_profile(path, 'sandbox', host='dl380g10', platform='linux/amd64',
                          podman=executable, environment={'HOME': str(root), 'XDG_RUNTIME_DIR': str(root)}, timeout=10)
    calls = [json.loads(line) for line in (root / 'calls').read_text().splitlines()]
    assert len(calls) == 1
    assert calls[0][:2] == ['image', 'exists']


def test_raw_profile_emits_exact_rust_prepare_command_without_final_schema(store, profile_map, monkeypatch):
    from tl3_harness import runtime_image

    root, executable = store
    path, entry, digest = profile_map
    actual = {'Id': entry['config_digest'], 'Os': 'linux', 'Architecture': 'amd64', 'RepoDigests': [entry['registry_ref']]}
    monkeypatch.setattr(runtime_image, 'preflight_profile', lambda *args, **kwargs:
                        {'profile': entry, 'map': {'path': str(path), 'sha256': digest, 'bytes': path.stat().st_size}, 'store': actual})
    prerequisites = {'no_cgroups_supported': False, 'init_available': False, 'user_scope_supported': False}
    config = runtime_image.prepare_runtime_profile(
        root / 'prepared', host_interpreter=sys.executable, podman=executable, systemctl='/explicit/systemctl',
        mode='own_scope', image_map=path, profile='sandbox', host='dl380g10', platform='linux/amd64',
        environment={'HOME': str(root), 'XDG_RUNTIME_DIR': str(root)}, producer_commit='a' * 40,
        prerequisites=prerequisites)
    command = json.loads(config.read_text())
    assert command['op'] == 'prepare_runtime_bundle'
    inputs = command['input']
    assert inputs['image_inventory'] == str(path)
    assert json.loads(Path(inputs['image_inspect']).read_bytes()) == actual
    assert inputs['prerequisites'] == prerequisites and inputs['mode'] == 'own_scope'
    code_root = Path(inputs['source_root'])
    inventory = json.loads((code_root / inputs['code_inventory']).read_bytes())
    assert set(inventory) == {'interpreter', 'files'}
    assert inventory['interpreter'] == sys.executable
    listed = {entry['path'] for entry in inventory['files']}
    listed.update({inputs['collector'], inputs['cleanup'], inputs['code_inventory']})
    assert {str(item.relative_to(code_root)) for item in code_root.rglob('*') if item.is_file()} == listed
    assert inputs['cleanup'] == 'host-cleanup'
    assert (code_root / inputs['trusted_root'] / 'runner.py').is_file()
    assert not list(config.parent.rglob('manifest.json'))
    assert config.stat().st_mode & 0o777 == 0o600


@pytest.mark.parametrize('mismatch', [False, True])
def test_actual_store_preflight_checks_ops_config_and_platform(store, profile_map, mismatch):
    from tl3_harness.runtime_image import preflight_profile

    root, executable = store
    path, entry, _ = profile_map
    (root / 'present').touch()
    (root / 'metadata').write_text(json.dumps({'Id': entry['config_digest'], 'Os': 'linux',
        'Architecture': 'arm64' if mismatch else 'amd64', 'RepoDigests': [entry['registry_ref']]}))
    def preflight():
        return preflight_profile(path, 'sandbox', host='dl380g10', platform='linux/amd64', podman=executable,
                                  environment={'HOME': str(root), 'XDG_RUNTIME_DIR': str(root)}, timeout=10)
    if mismatch:
        with pytest.raises(ValueError, match='differs from ops map'):
            preflight()
    else:
        assert preflight()['store']['Id'] == entry['config_digest']
