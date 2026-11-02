"""TL3 file tools. Every filesystem access starts at an open worktree handle."""

from contextlib import contextmanager, nullcontext
import errno
import glob as globlib
import json
import os
from pathlib import PurePosixPath
import re
import stat
from threading import Lock

import re2
from langchain_core.tools import StructuredTool, ToolException, create_schema_from_function

from tl3_workers.diff import make_diff_tool
from tl3_workers.pb_tests import make_test_tool
from tl3_workers.recall import make_recall_tool
from tl3_workers.memory import current


READ_TOOLS = frozenset({"list", "read", "glob", "grep", "diff"})
# Files larger than this are read by range and changed with edit (#321).
LARGE_FILE_BYTES = 65_536
WRITE_TOOLS = frozenset({"write", "edit"})
# Payload budgets leave room for pagination metadata and truncation notices.
OUTPUT_LIMITS = {
    "bytes": 16_384,
    "line_bytes": 2_048,
    "read_lines": 200,
    "grep_matches": 100,
    "glob_entries": 200,
    "list_entries": 200,
    # A regular expression sees this many characters of a line.
    "regex_line_chars": 1_048_576,
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
    # NAME_MAX per part. The total leaves room under PATH_MAX for the test
    # tool's staging directory, where a longer path would fail outside the tool.
    if len(path.encode("utf-8")) > 3072 or any(len(part.encode("utf-8")) > 255 for part in parts):
        raise ValueError("Path is too long")
    return parts


def _matching_lines(stream, pattern, regex=None, cut=None):
    """Yield (line number, preview, truncated) for matching lines.

    A regex is tried against the line up to regex_line_chars; cut counts the lines
    that ran past that.
    """
    number = 1
    prefix, tail, window = "", "", []
    length, kept, found = 0, 0, False
    cap = OUTPUT_LIMITS["line_bytes"]
    limit = OUTPUT_LIMITS["regex_line_chars"]

    def searched():
        text = "".join(window).rstrip("\r\n")
        return regex.search(text) is not None

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
        if regex is None:
            found |= pattern in tail + chunk
            tail = (tail + chunk)[-(len(pattern) - 1):] if len(pattern) > 1 else ""
        elif kept < limit:
            window.append(chunk[:limit - kept])
            kept += len(window[-1])
        if chunk.endswith(("\r", "\n")):
            ending = len(chunk) - len(chunk.rstrip("\r\n"))
            if regex is not None:
                found = searched()
                if cut is not None and length - ending > limit:
                    cut.append(number)
            if found:
                text = _clip(prefix.rstrip("\r\n"), cap)
                yield number, text, length - ending > len(text)
            number += 1
            prefix, tail, window = "", "", []
            length, kept, found = 0, 0, False
    if regex is not None and length:
        found = searched()
        if cut is not None and length > limit:
            cut.append(number)
    if length and found:
        text = _clip(prefix, cap)
        yield number, text, length > len(text)


# Errors the environment causes. ENOTDIR, ENOENT, ENAMETOOLONG, ELOOP and
# EISDIR follow from the model's own paths, so they never count (Q50).
ENVIRONMENT_ERRNOS = frozenset(
    errno.errorcode[code] for code in (
        errno.EACCES, errno.EPERM, errno.EROFS, errno.ENOSPC, errno.EDQUOT,
        errno.EIO, errno.EMFILE, errno.ENFILE, errno.ENOMEM,
    )
)


def environment_class(error):
    """The errno name of an environment failure, or None for the model's own input."""
    if error.errno is None:
        return "EACCES" if isinstance(error, PermissionError) else None
    name = errno.errorcode.get(error.errno)
    return name if name in ENVIRONMENT_ERRNOS else None


def _path_error(name, error, path):
    """Say what the model's own path did wrong and what to call instead."""
    path = _clip(str(path), 200)
    code = errno.errorcode.get(error.errno)
    if code == "ENOTDIR":
        # The component the walk named, when it is part of this call's path.
        file = error.filename if isinstance(error.filename, str) else ""
        file = file if path == file or path.startswith(file + "/") else path
        subject = f"{file} is a file, not a directory"
        if file != path:
            subject += f", so {path} is not a valid path"
        return (f"{subject}. To search a file, pass it to grep as path. To read part of it, "
                "use read with offset and limit. To see what a directory holds, use list.")
    if code == "EISDIR":
        return (f"{path} is a directory, not a file. Use list to see its entries, glob to match "
                "paths below it, or grep with it as path to search below it.")
    if code == "ELOOP":
        return f"{path} is or passes through a symbolic link. Tools never follow links; use a path that avoids it."
    if code == "ENAMETOOLONG":
        return f"{path} is too long. Use a shorter path."
    return (f"{name} could not be used on {path} ({code or 'no error code'}). "
            "Use list to check that the path exists and names what you meant.")


# grep reads its pattern as an RE2 regular expression. RE2 runs in time linear in the
# line, so no pattern can backtrack catastrophically and nothing here needs a clock.
# It has no backreferences or lookarounds, which are what make Python's re unsafe.
RE2_OPTIONS = re2.Options()
RE2_OPTIONS.log_errors = False
# Characters that mean something else as a regular expression than as written. A bare
# dot is left out: it matches itself too, so it cannot hide a literal match.
REGEX_SYNTAX = re.compile(r"[\\\[\](){}*+?^$|]")
_RE2_HINTS = (
    ("invalid perl operator", "RE2 has no lookarounds."),
    ("invalid repetition size", "RE2 limits repeat counts to 1,000."),
)


def _regex(pattern):
    """The compiled pattern and a note for the model; (None, note) means search it as text."""
    try:
        return re2.compile(pattern, RE2_OPTIONS), ""
    except re2.error as exc:
        detail = exc.args[0].decode("utf-8", "replace") if isinstance(exc.args[0], bytes) else str(exc)
    hint = next((hint for start, hint in _RE2_HINTS if detail.startswith(start)), None)
    if hint is None and re.search(r"\\[1-9]", detail):
        hint = "RE2 has no backreferences."
    hint = hint or "Escape special characters with a backslash to search them as a regular expression."
    return None, (f"Pattern is not a valid regular expression ({detail}), so it was searched as literal "
                  f"text. {hint} To search text as written, set literal=true.")


def _selectors_json(selectors):
    return None if selectors is None else [selectors[0], list(selectors[1])]


def _selectors(value):
    if value is None:
        return None
    base, tests = value
    if type(base) is not bool or not all(isinstance(test, str) for test in tests):
        raise ValueError("invalid evidence selectors")
    return (base, tuple(tests))


class Evidence:
    """Environment failures this run's tools recorded. A blocked reply must cite one (Q50)."""

    def __init__(self):
        self._lock = Lock()
        self.infrastructure_keys = {}
        self.tool_errors = {}

    def tool_error(self, name, cls, selectors=None):
        with self._lock:
            error = f"tool-error-{len(self.tool_errors) + 1}"
            self.tool_errors[error] = {"tool": name, "class": cls, "selectors": selectors}
            return error

    def test_result(self, result, selectors):
        if not isinstance(result, dict):
            return
        if result.get("status") == "completed":
            # A later complete run of the same tests shows the environment recovered.
            with self._lock:
                for key in [k for k, s in self.infrastructure_keys.items() if s == selectors]:
                    del self.infrastructure_keys[key]
                for error in self.tool_errors.values():
                    if error["selectors"] == selectors:
                        error["retired"] = True
            return
        if result.get("status") != "infrastructure_error" or result.get("origin") == "test_run":
            return
        key = result.get("action_key")
        if isinstance(key, str) and re.fullmatch("[0-9a-f]{64}", key):
            with self._lock:
                self.infrastructure_keys[key] = selectors
        else:
            result["tool_error"] = self.tool_error("run_tests", "test_submission", selectors)

    # A resumed maker visit keeps its evidence. JSON turns selector tuples into lists.
    def state(self):
        with self._lock:
            return {"infrastructure_keys": [[key, _selectors_json(s)] for key, s in self.infrastructure_keys.items()],
                    "tool_errors": {name: {**error, "selectors": _selectors_json(error["selectors"])}
                                    for name, error in self.tool_errors.items()}}

    def restore(self, state):
        keys = {key: _selectors(value) for key, value in state["infrastructure_keys"]}
        errors = {name: {**error, "selectors": _selectors(error["selectors"])}
                  for name, error in state["tool_errors"].items()}
        if not all(isinstance(key, str) for key in keys) or not all(
                isinstance(name, str) and isinstance(error.get("tool"), str) and isinstance(error.get("class"), str)
                for name, error in errors.items()):
            raise ValueError("invalid evidence state")
        with self._lock:
            self.infrastructure_keys, self.tool_errors = keys, errors

    def detail(self, evidence):
        """The worker-recorded facts behind cited evidence, or None if this run has none."""
        with self._lock:
            if evidence.get("action_key") in self.infrastructure_keys:
                return {"class": "test_infrastructure"}
            error = self.tool_errors.get(evidence.get("tool_error"))
            if error is None or error.get("retired"):
                return None
            return {"tool": error["tool"], "class": error["class"]}

    def cites(self, evidence):
        return self.detail(evidence) is not None


class Worktree:
    def __init__(self, root):
        self._root = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        self._mutation_lock = Lock()
        self._memory = current.get()
        self.evidence = Evidence()

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
            for depth, part in enumerate(parts, 1):
                if create:
                    try:
                        os.mkdir(part, dir_fd=fd)
                    except FileExistsError:
                        pass
                try:
                    child = os.open(
                        part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd,
                    )
                except NotADirectoryError:
                    # Name the component, so the tool guard can tell the model which one.
                    link = stat.S_ISLNK(os.stat(part, dir_fd=fd, follow_symlinks=False).st_mode)
                    code = errno.ELOOP if link else errno.ENOTDIR
                    raise OSError(code, os.strerror(code), "/".join(parts[:depth])) from None
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
                    if self._memory is not None:
                        try:
                            self._memory.file(path, write)
                        except Exception:
                            pass
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

    def _is_file(self, parts):
        if not parts:
            return False
        with self._directory(parts[:-1]) as parent:
            # Never follow a link: a symlink takes the directory path and fails there.
            return stat.S_ISREG(os.stat(parts[-1], dir_fd=parent, follow_symlinks=False).st_mode)

    def grep(self, pattern: str, path: str = ".", offset: int = 0, literal: bool = False) -> dict:
        """Find bounded matches of a regular expression, or of literal text with literal=true. Offset counts matches."""
        # Text with no regex syntax reads the same either way, so it keeps the exact
        # whole-line search; only a pattern that needs a regular expression pays for one.
        regex, note = _regex(pattern) if not literal and REGEX_SYNTAX.search(pattern) else (None, "")
        cut = []

        def in_file(relative):
            with self._file(relative) as stream:
                for number, preview, truncated in _matching_lines(stream, pattern, regex, cut):
                    if truncated:
                        preview += f" [Truncated. Use read offset={number - 1}, column=0.]"
                    yield {"path": relative, "line": number, "text": preview}

        def below(directory):
            for name, is_dir in self._walk(directory):
                if is_dir:
                    continue
                try:
                    yield from in_file(str(PurePosixPath(path) / name))
                except UnicodeDecodeError:
                    continue
        parts = _parts(path)
        if self._is_file(parts):
            # A named file that is not text is an error, not an empty result.
            result = _page(in_file(path), offset, OUTPUT_LIMITS["grep_matches"], "matches")
        else:
            with self._directory(parts) as directory:
                result = _page(below(directory), offset, OUTPUT_LIMITS["grep_matches"], "matches")
        if regex is not None and not offset and not result["matches"]:
            # The same text may have been meant as written: vars(digests) is a regex group.
            note = "No matches for the regular expression. To search for the text as written, set literal=true."
            if cut:
                limit = f"{OUTPUT_LIMITS['regex_line_chars']:,}"
                searched = (f"{len(cut)} line longer than {limit} characters was searched only in its first {limit}"
                            if len(cut) == 1 else
                            f"{len(cut)} lines longer than {limit} characters were searched only in their first {limit}")
                note += f" {searched} characters. literal=true searches whole lines."
        result["message"] = " ".join(filter(None, [note, result["message"]]))
        return result

    def write(self, path: str, content: str) -> str:
        """Write UTF-8 text, creating parent directories as needed."""
        content.encode("utf-8")
        with self._file(path, write=True, create=True) as stream:
            size = os.fstat(stream.fileno()).st_size
            if size > LARGE_FILE_BYTES:
                # A page read from a large file must not replace the whole file.
                raise ValueError(f"{path} is {size} bytes; change part of it with edit instead of write")
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
    "recall": make_recall_tool,
    "run_tests": make_test_tool,
    "list": lambda worktree: worktree.list_files,
    "read": lambda worktree: worktree.read,
    "glob": lambda worktree: worktree.glob,
    "grep": lambda worktree: worktree.grep,
    "diff": lambda worktree: make_diff_tool(worktree, _parts, OUTPUT_LIMITS),
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
        summary = current.get()
        lock = worktree._mutation_lock if name in WRITE_TOOLS | {"run_tests"} else nullcontext()

        def guarded(*args, _method=method, _lock=lock, _name=name, _summary=summary, **kwargs):
            with _lock:
                try:
                    result = _method(*args, **kwargs)
                    if _name == "run_tests":
                        selectors = (bool(kwargs.get("base")), tuple(sorted(set(kwargs.get("tests") or ()))))
                        worktree.evidence.test_result(result, selectors)
                    if _summary is not None:
                        try:
                            _summary.tool(_name, kwargs, result)
                        except Exception:
                            pass
                    return result
                except UnicodeError as exc:
                    raise ToolException("Expected valid UTF-8 text.") from exc
                except RecursionError as exc:
                    raise ToolException("The worktree is nested too deeply to walk.") from exc
                except ValueError as exc:
                    raise ToolException(str(exc)) from exc
                except FileNotFoundError as exc:
                    raise ToolException("File or directory not found.") from exc
                except OSError as exc:
                    cls = environment_class(exc)
                    if cls is None:
                        path = kwargs.get("path", ".") if _name in READ_TOOLS | WRITE_TOOLS else "the call's input"
                        raise ToolException(_path_error(_name, exc, path)) from exc
                    error = worktree.evidence.tool_error(_name, cls)
                    raise ToolException(
                        f"Worktree tool rejected the path or operation. Tool error id: {error}."
                    ) from exc

        tools.append(StructuredTool.from_function(
            func=guarded, name=name, description=description,
            args_schema=create_schema_from_function(name, method),
            handle_tool_error=True,
        ))
    return tools
