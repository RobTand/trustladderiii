"""Controlled store executables exercise streaming transfer without a daemon."""
import json
import os
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
    print('{{"Id":"sha256:fixture","Os":"linux","Architecture":"amd64","RepoDigests":[]}}')
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
