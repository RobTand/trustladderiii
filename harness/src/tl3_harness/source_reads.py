"""Q27 #855 option E: observe proof source reads only during pytest calls.

Loaded by path from the trusted runner mount; keep this sibling stdlib-only.
This is an in-process check. Independent review still covers deliberate
interpreter tampering and code-object inspection.
"""
import importlib.util
import os
from pathlib import Path
import sys


class SubjectSourceRead(RuntimeError):
    pass


class SourceReads:
    def __init__(self, root):
        self.root = Path(root).resolve()
        self.subjects = set()
        self.observer = None
        self.identities = {}
        self.violation = None
        self.active = False
        sys.addaudithook(self.audit)
        # Descriptor reads have no CPython audit event. Check them as well so a
        # descriptor opened in setup cannot hide a read during the test call.
        for name in ('read', 'pread', 'readv', 'preadv'):
            original = getattr(os, name, None)
            if original is not None:
                setattr(os, name, self.descriptor_reader(original))

    def descriptor_reader(self, original):
        def read(descriptor, *args, **kwargs):
            if self.active:
                self.record(descriptor)
            return original(descriptor, *args, **kwargs)
        return read

    def begin(self, node, subjects):
        self.violation = None
        self.subjects = {self.path(name) for name in subjects}
        self.observer = self.path(node.split('::', 1)[0])
        self.identities = {}
        for subject in self.subjects:
            path = Path(subject)
            caches = path.parent / '__pycache__'
            candidates = [path, path.with_suffix('.pyc'), *caches.glob(path.stem + '.*.pyc')]
            for candidate in candidates:
                try:
                    info = candidate.stat()
                except OSError:
                    continue
                self.identities[info.st_dev, info.st_ino] = str(candidate)
        self.active = bool(self.subjects)

    def finish(self):
        self.active = False
        violation, self.violation = self.violation, None
        self.subjects = set()
        self.identities = {}
        if violation is not None:
            raise SubjectSourceRead(violation)

    def path(self, value):
        return os.path.realpath(os.fsdecode(value))

    def subject_path(self, value):
        if isinstance(value, int):
            try:
                info = os.fstat(value)
            except OSError:
                return None
            return self.identities.get((info.st_dev, info.st_ino))
        if not isinstance(value, (str, bytes, os.PathLike)):
            return None
        path = self.path(value)
        source = path
        if path.endswith('.pyc'):
            try:
                source = importlib.util.source_from_cache(path)
            except ValueError:
                source = path[:-1]
        return path if source in self.subjects else None

    def record(self, value):
        if self.violation is not None:
            return
        target = self.subject_path(value)
        if target is None:
            return
        frame = sys._getframe(1)
        while frame is not None:
            filename = frame.f_code.co_filename
            # A normal import may read source or bytecode on the observer's
            # stack. get_source used by inspect/linecache is deliberately not
            # exempt: it is an observer read, not module loading.
            if (frame.f_globals.get('__name__') in ('_frozen_importlib', '_frozen_importlib_external',
                                                  'importlib._bootstrap', 'importlib._bootstrap_external')
                    and frame.f_code.co_name in ('get_code', 'exec_module')):
                return
            if not filename.startswith('<'):
                path = self.path(filename)
                if Path(path).is_relative_to(self.root):
                    if path in self.subjects and path != self.observer:
                        return
                    reader = Path(path).relative_to(self.root).as_posix()
                    subject = Path(target).relative_to(self.root).as_posix()
                    self.violation = (f"subject_source_read: {subject}, reader {reader}:{frame.f_lineno}; "
                                      "assert on behaviour; don't read subject source")
                    return
            frame = frame.f_back

    def audit(self, event, args):
        if not self.active:
            return
        if event == 'open':
            path, mode, flags = args
            readable = ('r' in mode or '+' in mode) if mode is not None else (flags & os.O_ACCMODE) != os.O_WRONLY
            if readable:
                self.record(path)
        elif event == 'mmap.__new__' and args[0] != -1:
            self.record(args[0])
