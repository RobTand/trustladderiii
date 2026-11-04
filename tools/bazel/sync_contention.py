"""Opt-in storage-barrier forcing for remote pytest contention reproductions."""
import json
import os
import time


class SyncContention:
    def __init__(self, delay):
        self.delay = delay
        self.originals = {}
        self.calls = {name: 0 for name in ('fsync', 'fdatasync')}
        self.seconds = dict.fromkeys(self.calls, 0.0)

    def pytest_sessionstart(self):
        print(f'SYNC_CONTENTION delay_seconds={self.delay}', flush=True)
        for name in self.calls:
            original = getattr(os, name)
            self.originals[name] = original

            def sync(fd, name=name, original=original):
                started = time.monotonic()
                self.calls[name] += 1
                if self.calls[name] == 1:
                    print(f'SYNC_CONTENTION first {name}', flush=True)
                try:
                    # Model a slow shared disk; this is forcing, never a retry.
                    time.sleep(self.delay)
                    return original(fd)
                finally:
                    self.seconds[name] += time.monotonic() - started

            setattr(os, name, sync)

    def pytest_sessionfinish(self):
        for name, original in self.originals.items():
            setattr(os, name, original)
        print('SYNC_CONTENTION ' + json.dumps({
            'delay_seconds': self.delay, 'calls': self.calls, 'seconds': self.seconds}))
