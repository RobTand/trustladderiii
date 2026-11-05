"""#825: exited orphans must not consume the container's 128 PID slots."""
import errno
import os
from pathlib import Path
import select
import threading
import time

import pytest


def test_exited_orphans_leave_room_for_teardown():
    orphans = []
    descriptors = set()
    reaper = select.poll()

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

    def wait_for_reap(deadline):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            exhausted(errno.EAGAIN)
        events = reaper.poll(int(remaining * 1000))
        freed = False
        for descriptor, event in events:
            # POLLIN proves exit; only POLLHUP proves the PID slot was released.
            if event & select.POLLHUP:
                reaper.unregister(descriptor)
                descriptors.remove(descriptor)
                os.close(descriptor)
                freed = True
        if not freed:
            exhausted(errno.EAGAIN)

    def create_orphan():
        ready_read, ready_write = os.pipe()
        release_read, release_write = os.pipe()
        try:
            parent = os.fork()
        except OSError:
            for descriptor in (ready_read, ready_write, release_read, release_write):
                os.close(descriptor)
            raise
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
                error = int(ready.split(':')[1])
                raise OSError(error, os.strerror(error))
            orphan = int(ready)
            # The grandchild cannot exit until its parent has been reaped.
            descriptor = os.pidfd_open(orphan)
            os.write(release_write, b'x')
            return orphan, descriptor
        finally:
            os.close(ready_read)
            os.close(release_write)

    try:
        for _ in range(160):
            deadline = time.monotonic() + 10
            while True:
                try:
                    orphan, descriptor = create_orphan()
                except OSError as error:
                    if error.errno != errno.EAGAIN:
                        raise
                    wait_for_reap(deadline)
                    continue
                orphans.append(orphan)
                descriptors.add(descriptor)
                reaper.register(descriptor, select.POLLHUP)
                assert select.select([descriptor], [], [], 10)[0], 'orphan did not exit'
                break
        deadline = time.monotonic() + 10
        while descriptors:
            wait_for_reap(deadline)
    finally:
        for descriptor in descriptors:
            os.close(descriptor)

    completed = []
    teardown = threading.Thread(target=lambda: completed.append(True))
    teardown.start()
    teardown.join()
    assert completed == [True]
