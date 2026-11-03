from pathlib import Path

from tl3_harness.execution import Container, container_command


def test_pytest_uses_private_disk_scratch_in_the_shared_command():
    command = container_command(Path('/served'), Container('sha256:' + '1' * 64), ())
    assert '--tmpfs' not in command, 'pytest scratch must be disk-backed, as in served CI'
    mounts = [command[i + 1] for i, arg in enumerate(command) if arg == '--mount']
    assert len(mounts) == 3
    assert 'type=bind,src=/served,dst=/checkout,readonly' in mounts
    assert any(mount.startswith('type=bind,src=') and mount.endswith(',dst=/tmp') for mount in mounts)
    assert '--read-only' in command and '--network=none' in command
    assert '--cap-drop=ALL' in command and '--user=65534:65534' in command
