from dataclasses import fields
from pathlib import Path

import pytest

from tl3_harness import Container
from tl3_harness.execution import TMPFS_MIB, container_command


@pytest.mark.parametrize('enabled', [False, True])
def test_executable_scratch_is_explicit_and_keeps_the_boundary(enabled):
    assert 'exec_scratch' in {field.name for field in fields(Container)}, 'scratch needs an explicit opt-in'
    container = Container('sha256:' + '1' * 64, exec_scratch=enabled)
    command = container_command(Path('/served'), container, ())
    mount = command[command.index('--tmpfs') + 1]
    assert ('exec' if enabled else 'noexec') in mount.split(':', 1)[1].split(',')
    assert f'size={TMPFS_MIB}m' in mount and TMPFS_MIB == 512
    assert 'nosuid' in mount and 'nodev' in mount
    assert '--read-only' in command and '--network=none' in command
    assert '--cap-drop=ALL' in command and '--user=65534:65534' in command
    assert sum(argument == '--mount' for argument in command) == 2


def test_default_scratch_is_not_executable():
    assert getattr(Container('sha256:' + '1' * 64), 'exec_scratch', None) is False


@pytest.mark.parametrize('invalid', ['true', 1, None])
def test_scratch_opt_in_requires_a_boolean(invalid):
    assert 'exec_scratch' in {field.name for field in fields(Container)}, 'scratch needs an explicit opt-in'
    with pytest.raises(ValueError, match='boolean'):
        Container('sha256:' + '1' * 64, exec_scratch=invalid)
