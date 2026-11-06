"""Controlled store executables exercise streaming transfer without a daemon."""
import json
import os
import sys

import pytest


@pytest.fixture
def store(tmp_path):
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
                            environment={'PATH': os.defpath}, log=root / 'import.log', timeout=10,
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
                     environment={'PATH': os.defpath}, log=root / 'import.log', timeout=10,
                     validate=reject)
    commands = [json.loads(line) for line in (root / 'calls').read_text().splitlines()]
    assert all(command[0] != 'save' and command != ['load'] for command in commands)
