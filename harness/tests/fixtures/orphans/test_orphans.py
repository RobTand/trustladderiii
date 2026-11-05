"""#825: exited orphans must not consume the container's 128 PID slots."""
import errno
import os
from pathlib import Path
import select
import threading

import pytest


def test_exited_orphans_leave_room_for_teardown():
    orphans = []

    def exhausted(error):
        states = []
        for pid in orphans:
            try:
                state, parent, *_ = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
                states.append((state, int(parent)))
            except FileNotFoundError:
                pass
        zombies = sum(state == 'Z' and parent == 1 for state, parent in states)
        live = sum(state != 'Z' for state, _ in states)
        assert error == errno.EAGAIN, error
        assert live == 0, f'LIVE orphan leak: {live} live, {zombies} zombies'
        pytest.fail(f'PID exhaustion after {len(orphans)} exited orphans: {zombies} zombies reparented to PID 1, {live} live')

    for _ in range(160):
        ready_read, ready_write = os.pipe()
        release_read, release_write = os.pipe()
        try:
            parent = os.fork()
        except OSError as error:
            exhausted(error.errno)
        if parent == 0:
            os.close(ready_read)
            os.close(release_write)
            try:
                orphan = os.fork()
            except OSError as error:
                os.write(ready_write, f'error:{error.errno}'.encode())
                os._exit(1)
            if orphan == 0:
                os.close(ready_write)
                os.read(release_read, 1)
                os._exit(0)
            os.write(ready_write, str(orphan).encode())
            os._exit(0)
        os.close(ready_write)
        os.close(release_read)
        try:
            ready = os.read(ready_read, 64).decode()
            os.waitpid(parent, 0)
            if ready.startswith('error:'):
                exhausted(int(ready.split(':')[1]))
            orphan = int(ready)
            orphans.append(orphan)
            # The grandchild cannot exit until its parent has been reaped.
            descriptor = os.pidfd_open(orphan)
            try:
                os.write(release_write, b'x')
                assert select.select([descriptor], [], [], 10)[0], 'orphan did not exit'
            finally:
                os.close(descriptor)
        finally:
            os.close(ready_read)
            os.close(release_write)

    completed = []
    teardown = threading.Thread(target=lambda: completed.append(True))
    teardown.start()
    teardown.join()
    assert completed == [True]
