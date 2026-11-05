"""Observe real writer syscalls and the filesystem at each process-crash cut."""
from contextlib import ExitStack
import io
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from tl3_workers import facts, memory


class Crash(BaseException):
    pass


class Disk:
    def __init__(self, root, crash=0):
        self.root = root
        self.crash = crash
        self.calls = []
        self.points = 0
        self.frozen = None
        self.serial = 0
        self.original_open = Path.open

    def name(self, path):
        return str(Path(path).relative_to(self.root))

    def snapshot(self):
        # These are only the files created by the two real publishers in this fixture.
        return {p.name: p.read_bytes() for p in sorted(self.root.iterdir()) if p.is_file()}

    def note(self, *event):
        if self.frozen is None:
            self.calls.append(event)

    def point(self, *event):
        if self.frozen is not None:
            return
        self.calls.append(event)
        self.points += 1
        if self.points == self.crash:
            self.frozen = self.snapshot()
            raise Crash()

    def result(self):
        return self.calls, self.snapshot() if self.frozen is None else self.frozen

    def uuid(self):
        self.serial += 1
        return SimpleNamespace(hex=f'{self.serial:032x}')

    def install(self, stack):
        disk = self
        original_fsync, original_replace = facts.os.fsync, facts.os.replace
        original_os_open = facts.os.open
        descriptors = {}

        class Raw(io.FileIO):
            def write(self, data):
                count = super().write(data)
                disk.point('write', disk.name(self.name), bytes(data[:count]))
                return count

        class Handle:
            def __init__(self, handle, path):
                self.handle, self.path = handle, path

            def __getattr__(self, name):
                return getattr(self.handle, name)

            def write(self, data):
                return self.handle.write(data)

            def flush(self):
                self.handle.flush()
                disk.point('flush', disk.name(self.path))

            def __enter__(self):
                return self

            def __exit__(self, *args):
                return self.handle.__exit__(*args)

        def opened(path, mode='r', *args, **kwargs):
            if path.parent != disk.root or mode not in {'x', 'xb'}:
                return disk.original_open(path, mode, *args, **kwargs)
            raw = Raw(path, mode)
            descriptors[raw.fileno()] = disk.name(path)
            buffered = io.BufferedWriter(raw)
            handle = buffered if 'b' in mode else io.TextIOWrapper(buffered)
            disk.note('open', disk.name(path), mode)
            return Handle(handle, path)

        def directory_opened(path, flags, *args, **kwargs):
            fd = original_os_open(path, flags, *args, **kwargs)
            if Path(path) == disk.root:
                descriptors[fd] = 'directory'
                disk.note('open-directory')
            return fd

        def synced(fd):
            original_fsync(fd)
            disk.point('fsync', descriptors.get(fd, 'directory'))

        def renamed(source, destination):
            original_replace(source, destination)
            disk.point('rename', disk.name(source), disk.name(destination))

        stack.enter_context(patch.object(Path, 'open', opened))
        stack.enter_context(patch.object(facts.os, 'open', directory_opened))
        stack.enter_context(patch.object(facts.os, 'fsync', synced))
        stack.enter_context(patch.object(facts.os, 'replace', renamed))
        for module in [facts, memory]:
            stack.enter_context(patch.object(module, 'uuid4', self.uuid))
        return self

    def __enter__(self):
        self.stack = ExitStack()
        return self.install(self.stack)

    def __exit__(self, *args):
        return self.stack.__exit__(*args)
