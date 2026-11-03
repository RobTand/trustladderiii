from pathlib import Path

from tl3_harness.execution import Container, container_command


def test_pytest_uses_private_disk_scratch_in_the_shared_command(tmp_path, monkeypatch):
    monkeypatch.setattr('tl3_harness.execution.scratch_directory', lambda: tmp_path)
    command = container_command(Path('/served'), Container('sha256:' + '1' * 64), ())
    assert '--tmpfs' not in command, 'pytest scratch must be disk-backed, as in served CI'
    mounts = [command[i + 1] for i, arg in enumerate(command) if arg == '--mount']
    assert len(mounts) == 3
    assert 'type=bind,src=/served,dst=/checkout,readonly' in mounts
    assert any(mount.startswith('type=bind,src=') and mount.endswith(',dst=/tmp') for mount in mounts)
    assert '--read-only' in command and '--network=none' in command
    owner = tmp_path.parent.stat()
    assert '--cap-drop=ALL' in command and f'--user={owner.st_uid}:{owner.st_gid}' in command


def test_each_container_gets_a_private_child_of_the_action_leaf(tmp_path, monkeypatch):
    from tl3_harness import scratch

    monkeypatch.setattr(scratch, 'registered_leaf', lambda: tmp_path)
    first, second = scratch.directory(), scratch.directory()
    assert first != second
    assert first.parent == second.parent == tmp_path
    assert first.stat().st_mode & 0o777 == second.stat().st_mode & 0o777 == 0o700
