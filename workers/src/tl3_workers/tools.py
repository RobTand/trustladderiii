"""TL3 file tools. Every filesystem access starts at an open worktree handle."""

from contextlib import contextmanager
import glob as globlib
import json
import os
from pathlib import PurePosixPath
import re
import stat

from langchain_core.tools import StructuredTool, ToolException, create_schema_from_function

from tl3_workers.pb_tests import make_test_tool


READ_TOOLS = frozenset({"list", "read", "glob", "grep"})
WRITE_TOOLS = frozenset({"write", "edit"})
# Payload budgets leave room for pagination metadata and truncation notices.
OUTPUT_LIMITS = {
    "bytes": 16_384,
    "line_bytes": 2_048,
    "read_lines": 200,
    "grep_matches": 100,
    "glob_entries": 200,
    "list_entries": 200,
}


def _clip(text, budget):
    return text.encode("utf-8")[:budget].decode("utf-8", errors="ignore")


def _page(items, offset, cap, label):
    if offset < 0:
        raise ValueError("Offset must be nonnegative")
    entries = []
    used = 0
    for index, item in enumerate(items):
        if index < offset:
            continue
        size = len(json.dumps(item, ensure_ascii=True).encode("utf-8")) + 2
        if len(entries) == cap or used + size > OUTPUT_LIMITS["bytes"] - 512:
            next_offset = offset + len(entries)
            message = f"Truncated. Continue with offset={next_offset}."
            if not entries:
                message = "Truncated. Entry exceeds the byte cap; narrow the path or pattern."
                next_offset = None
            return {label: entries, "next_offset": next_offset, "message": message}
        entries.append(item)
        used += size
    return {label: entries, "next_offset": None, "message": ""}


def _parts(path):
    if not path or "\x00" in path or "\\" in path or path.startswith("/"):
        raise ValueError("Use a relative worktree path")
    if ".." in path.split("/"):
        raise ValueError("Parent traversal is forbidden")
    parts = PurePosixPath(path).parts
    if ".git" in parts:
        raise ValueError("Git metadata paths are forbidden")
    return parts


def _matching_lines(stream, pattern):
    number = 1
    prefix, tail = "", ""
    length, found = 0, False
    cap = OUTPUT_LIMITS["line_bytes"]
    while chunk := stream.readline(cap + 1):
        # newline="" preserves CRLF, including when readline's cap splits it.
        if chunk.endswith("\r"):
            position = stream.tell()
            if stream.read(1) == "\n":
                chunk += "\n"
            else:
                stream.seek(position)
        prefix = (prefix + chunk)[:cap + 1]
        length += len(chunk)
        found |= pattern in tail + chunk
        tail = (tail + chunk)[-(len(pattern) - 1):] if len(pattern) > 1 else ""
        if chunk.endswith(("\r", "\n")):
            if found:
                text = _clip(prefix.rstrip("\r\n"), cap)
                ending = len(chunk) - len(chunk.rstrip("\r\n"))
                yield number, text, length - ending > len(text)
            number += 1
            prefix, tail = "", ""
            length, found = 0, False
    if length and found:
        text = _clip(prefix, cap)
        yield number, text, length > len(text)


class Worktree:
    def __init__(self, root):
        self._root = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        os.close(self._root)

    @contextmanager
    def _directory(self, parts, *, create=False):
        # A dup shares its directory position with every other dup, so parallel
        # tool calls would steal each other's entries. Open the root afresh.
        fd = os.open(".", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=self._root)
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
            try:
                fd = os.open(
                    parts[-1], flags | os.O_NOFOLLOW | os.O_NONBLOCK, 0o644, dir_fd=parent,
                )
            except FileNotFoundError:
                raise ValueError("File not found") from None
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
            try:
                _parts(name)
            except ValueError:
                continue
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

    def list_files(self, path: str = ".", offset: int = 0) -> dict:
        """List bounded direct children. Continue a truncated result with offset."""
        with self._directory(_parts(path)) as directory:
            return _page(
                ({"path": str(PurePosixPath(path) / name), "is_dir": is_dir}
                 for name, is_dir in self._entries(directory)),
                offset, OUTPUT_LIMITS["list_entries"], "entries",
            )

    def read(self, path: str, offset: int = 0, limit: int = 200, column: int = 0) -> str:
        """Read bounded UTF-8 text. Offset is zero-based lines, column is characters."""
        if offset < 0 or limit < 1 or column < 0:
            raise ValueError("Invalid line range")
        chunks = []
        used = 0
        with self._file(path) as stream:
            for _ in range(offset):
                while chunk := stream.readline(OUTPUT_LIMITS["line_bytes"]):
                    if chunk.endswith("\n"):
                        break
                    if chunk.endswith("\r"):
                        position = stream.tell()
                        if stream.read(1) != "\n":
                            stream.seek(position)
                        break
                if not chunk:
                    return ""
            # Skip a column without allocating an unbounded line.
            remaining = column
            while remaining:
                chunk = stream.readline(min(remaining, OUTPUT_LIMITS["line_bytes"]))
                remaining -= len(chunk)
                if not chunk or chunk.endswith(("\r", "\n")):
                    raise ValueError("Column exceeds line length")
            for number in range(min(limit, OUTPUT_LIMITS["read_lines"])):
                chunk = stream.readline(OUTPUT_LIMITS["line_bytes"] + 1)
                if not chunk:
                    return "".join(chunks)
                budget = min(OUTPUT_LIMITS["line_bytes"], OUTPUT_LIMITS["bytes"] - 256 - used)
                text = _clip(chunk, budget)
                if text.endswith("\r") and chunk[len(text):].startswith("\n"):
                    text = text[:-1]
                if not text:
                    next_line, next_column = offset + number, column if number == 0 else 0
                    break
                chunks.append(text)
                used += len(text.encode("utf-8"))
                if len(text) < len(chunk) or not text.endswith(("\r", "\n")):
                    # A short final line needs no continuation notice.
                    if text == chunk and not stream.read(1):
                        return "".join(chunks)
                    next_line = offset + number
                    next_column = (column if number == 0 else 0) + len(text)
                    break
                next_line, next_column = offset + number + 1, 0
            else:
                if not stream.read(1):
                    return "".join(chunks)
            return "".join(chunks) + (
                f"\n[Truncated. Continue with offset={next_line}, column={next_column}.]"
            )

    def glob(self, pattern: str, path: str = ".", offset: int = 0) -> dict:
        """Match bounded relative paths. Use ** recursively and offset to continue."""
        _parts(pattern)
        matcher = re.compile(globlib.translate(pattern, recursive=True, include_hidden=True))
        with self._directory(_parts(path)) as directory:
            return _page(
                (str(PurePosixPath(path) / name)
                 for name, _ in self._walk(directory) if matcher.fullmatch(name)),
                offset, OUTPUT_LIMITS["glob_entries"], "entries",
            )

    def grep(self, pattern: str, path: str = ".", offset: int = 0) -> dict:
        """Find bounded literal matches. Offset counts matches; text previews may truncate."""
        def matches(directory):
            for name, is_dir in self._walk(directory):
                if is_dir:
                    continue
                relative = str(PurePosixPath(path) / name)
                try:
                    with self._file(relative) as stream:
                        for number, preview, truncated in _matching_lines(stream, pattern):
                            if truncated:
                                preview += f" [Truncated. Use read offset={number - 1}, column=0.]"
                            yield {"path": relative, "line": number, "text": preview}
                except UnicodeDecodeError:
                    continue
        with self._directory(_parts(path)) as directory:
            return _page(matches(directory), offset, OUTPUT_LIMITS["grep_matches"], "matches")

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
            if not count:
                raise ValueError("No match found")
            if count != 1 and not replace_all:
                raise ValueError("Expected a unique match, or replace_all")
            stream.seek(0)
            stream.write(content.replace(old, new))
            stream.truncate()
        return f"Replaced {count} occurrence(s)."


# Never expose execute or a local shell.
TOOL_FACTORIES = {
    "run_tests": make_test_tool,
    "list": lambda worktree: worktree.list_files,
    "read": lambda worktree: worktree.read,
    "glob": lambda worktree: worktree.glob,
    "grep": lambda worktree: worktree.grep,
    "write": lambda worktree: worktree.write,
    "edit": lambda worktree: worktree.edit,
}


def build_tools(worktree, descriptions):
    unknown = descriptions.keys() - TOOL_FACTORIES.keys()
    if unknown:
        raise ValueError(f"Unknown TL3 tools: {sorted(unknown)}")
    tools = []
    for name, description in descriptions.items():
        if not description.strip():
            raise ValueError("Each role must describe its tools")
        method = TOOL_FACTORIES[name](worktree)

        def guarded(*args, _method=method, **kwargs):
            try:
                return _method(*args, **kwargs)
            except UnicodeError as exc:
                raise ToolException("Expected valid UTF-8 text.") from exc
            except ValueError as exc:
                raise ToolException(str(exc)) from exc
            except FileNotFoundError as exc:
                raise ToolException("File or directory not found.") from exc
            except OSError as exc:
                raise ToolException("Worktree tool rejected the path or operation.") from exc

        tools.append(StructuredTool.from_function(
            func=guarded, name=name, description=description,
            args_schema=create_schema_from_function(name, method),
            handle_tool_error=True,
        ))
    return tools
