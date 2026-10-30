"""TL3 file tools. Every filesystem access starts at an open worktree handle."""

from contextlib import contextmanager
import glob as globlib
from itertools import islice
import os
from pathlib import PurePosixPath
import re
import stat

from langchain_core.tools import StructuredTool, ToolException, create_schema_from_function


READ_TOOLS = frozenset({"list", "read", "glob", "grep"})
WRITE_TOOLS = frozenset({"write", "edit"})
# Ticket #33 can add its admitted PrismaBuild tool factory here. Never add execute.
TOOL_NAMES = READ_TOOLS | WRITE_TOOLS


def _parts(path):
    if not path or "\x00" in path or "\\" in path or path.startswith("/"):
        raise ValueError("Use a relative worktree path")
    if ".." in path.split("/"):
        raise ValueError("Parent traversal is forbidden")
    return PurePosixPath(path).parts


class Worktree:
    def __init__(self, root):
        self._root = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        os.close(self._root)

    @contextmanager
    def _directory(self, parts, *, create=False):
        fd = os.dup(self._root)
        try:
            for part in parts:
                if create:
                    try:
                        os.mkdir(part, dir_fd=fd)
                    except FileExistsError:
                        pass
                child = os.open(
                    part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd,
                )
                os.close(fd)
                fd = child
            yield fd
        finally:
            os.close(fd)

    @contextmanager
    def _file(self, path, *, write=False, create=False):
        parts = _parts(path)
        if not parts:
            raise ValueError("Expected a file path")
        with self._directory(parts[:-1], create=create) as parent:
            flags = os.O_RDWR if write else os.O_RDONLY
            if create:
                flags |= os.O_CREAT
            fd = os.open(
                parts[-1], flags | os.O_NOFOLLOW | os.O_NONBLOCK, 0o644, dir_fd=parent,
            )
            try:
                info = os.fstat(fd)
                # Never open devices or FIFOs as text, or modify an external hard link.
                if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
                    raise ValueError("Expected a regular file with one link")
                stream = os.fdopen(fd, "r+" if write else "r", encoding="utf-8", newline="")
                fd = -1
                with stream:
                    yield stream
            finally:
                if fd >= 0:
                    os.close(fd)

    def _entries(self, directory):
        for name in sorted(os.listdir(directory)):
            info = os.stat(name, dir_fd=directory, follow_symlinks=False)
            if stat.S_ISDIR(info.st_mode):
                yield name, True
            elif stat.S_ISREG(info.st_mode) and info.st_nlink == 1:
                yield name, False

    def _walk(self, directory, prefix=""):
        for name, is_dir in self._entries(directory):
            path = prefix + name
            yield path, is_dir
            if is_dir:
                child = os.open(
                    name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=directory,
                )
                try:
                    yield from self._walk(child, path + "/")
                finally:
                    os.close(child)

    def list_files(self, path: str = ".") -> list[dict]:
        """List direct children of a relative directory. Symlinks are omitted."""
        with self._directory(_parts(path)) as directory:
            return [{"path": str(PurePosixPath(path) / name), "is_dir": is_dir}
                    for name, is_dir in self._entries(directory)]

    def read(self, path: str, offset: int = 0, limit: int = 200) -> str:
        """Read UTF-8 text with a zero-based line offset and a positive line limit."""
        if offset < 0 or limit < 1:
            raise ValueError("Invalid line range")
        with self._file(path) as stream:
            return "".join(islice(stream, offset, offset + limit))

    def glob(self, pattern: str, path: str = ".") -> list[str]:
        """Match relative paths under a directory. Use ** to search recursively."""
        _parts(pattern)
        matcher = re.compile(globlib.translate(pattern, recursive=True, include_hidden=True))
        with self._directory(_parts(path)) as directory:
            return [str(PurePosixPath(path) / name)
                    for name, _ in self._walk(directory) if matcher.fullmatch(name)]

    def grep(self, pattern: str, path: str = ".") -> list[dict]:
        """Find literal text in UTF-8 files below a relative directory."""
        matches = []
        with self._directory(_parts(path)) as directory:
            for name, is_dir in self._walk(directory):
                if is_dir:
                    continue
                relative = str(PurePosixPath(path) / name)
                try:
                    with self._file(relative) as stream:
                        for number, line in enumerate(stream, 1):
                            if pattern in line:
                                matches.append({"path": relative, "line": number,
                                                "text": line.rstrip("\r\n")})
                except UnicodeDecodeError:
                    continue
        return matches

    def write(self, path: str, content: str) -> str:
        """Write UTF-8 text, creating parent directories as needed."""
        content.encode("utf-8")
        with self._file(path, write=True, create=True) as stream:
            stream.write(content)
            stream.truncate()
        return "File written."

    def edit(self, path: str, old: str, new: str, replace_all: bool = False) -> str:
        """Replace exact text. The match must be unique unless replace_all is true."""
        new.encode("utf-8")
        if not old:
            raise ValueError("Expected nonempty text to replace")
        with self._file(path, write=True) as stream:
            content = stream.read()
            count = content.count(old)
            if not count or (count != 1 and not replace_all):
                raise ValueError("Expected a unique match, or replace_all")
            stream.seek(0)
            stream.write(content.replace(old, new))
            stream.truncate()
        return f"Replaced {count} occurrence(s)."


def build_tools(worktree, descriptions):
    unknown = descriptions.keys() - TOOL_NAMES
    if unknown:
        raise ValueError(f"Unknown TL3 tools: {sorted(unknown)}")
    tools = []
    for name, description in descriptions.items():
        if not description.strip():
            raise ValueError("Each role must describe its tools")
        method = worktree.list_files if name == "list" else getattr(worktree, name)

        def guarded(*args, _method=method, **kwargs):
            try:
                return _method(*args, **kwargs)
            except (OSError, ValueError) as exc:
                raise ToolException("Worktree tool rejected the path or operation.") from exc

        tools.append(StructuredTool.from_function(
            func=guarded, name=name, description=description,
            args_schema=create_schema_from_function(name, method),
            handle_tool_error=True,
        ))
    return tools
