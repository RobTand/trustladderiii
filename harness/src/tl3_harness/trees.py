import ast
import os
from pathlib import Path, PurePosixPath
import subprocess
import tomllib


def git(repo: Path, *args: str) -> bytes:
    return subprocess.run(
        ["git", "--no-replace-objects", "-c", "core.hooksPath=/dev/null",
         "-c", "credential.helper=", "-c", "protocol.allow=never",
         "-c", "protocol.file.allow=always", "-c", "core.fsmonitor=false",
         "-c", "commit.gpgsign=false", "-C", str(repo), *args],
        env={**{key: value for key, value in os.environ.items() if not key.startswith("GIT_")},
             "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
             "GIT_TERMINAL_PROMPT": "0", "GIT_NO_LAZY_FETCH": "1",
             "GIT_EDITOR": "true", "GIT_SEQUENCE_EDITOR": "true"},
        check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    ).stdout


def commit(repo: Path, ref: str) -> str:
    return git(repo, "rev-parse", "--verify", "--end-of-options", f"{ref}^{{commit}}").decode().strip()


class UnsupportedEntry(ValueError):
    """A tree entry checks never run: a symlink, a submodule or another special file."""

    def __init__(self, name: str):
        super().__init__(f"check trees must contain only regular files and directories: {name}")
        self.name = name


def export(repo: Path, revision: str, target: Path):
    target.mkdir()
    for entry in git(repo, "ls-tree", "-rz", revision).split(b"\x00"):
        if not entry:
            continue
        metadata, raw_name = entry.split(b"\t", 1)
        mode, kind, oid = metadata.split()
        # A served checkout may contain a symlink to a host credential or socket.
        if mode not in (b"100644", b"100755") or kind != b"blob":
            raise UnsupportedEntry(raw_name.decode(errors="backslashreplace"))
        path = target / raw_name.decode()
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(git(repo, "cat-file", "blob", oid.decode()))
        path.chmod(0o755 if mode == b"100755" else 0o644)


def configuration(tree: Path) -> dict:
    with (tree / "pyproject.toml").open("rb") as source:
        config = tomllib.load(source)["tool"]["tl3"]["harness"]
    if set(config) != {"command", "test-files"}:
        raise ValueError("harness configuration needs command and test-files")
    command, files = config["command"], config["test-files"]
    if command not in (["python", "-m", "pytest"], ["python3", "-m", "pytest"]):
        raise ValueError("slice one supports the Python pytest module command")
    if not isinstance(files, list) or not files or len(set(files)) != len(files):
        raise ValueError("test-files must list the repo's test files")
    for name in files:
        if not isinstance(name, str):
            raise ValueError("test-files must contain strings")
        path = PurePosixPath(name)
        if (path.is_absolute() or ".." in path.parts
                or "\\" in name or path.name == "conftest.py" or path.suffix != ".py"):
            raise ValueError("test-files must be relative Python test paths")
    return config


def test_edits(base: Path, head: Path, destination: Path, files: list[str]):
    names = {path.relative_to(tree).as_posix() for tree in (base, head)
             for path in tree.rglob("*.py")
             if path.name != "conftest.py" and is_test(path.relative_to(tree).as_posix(), files)}
    for name in names:
        old, new, target = base / name, head / name, destination / name
        if new.exists():
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(new.read_bytes())
        elif old.exists():
            target.unlink()


def is_test(name: str, patterns: list[str]) -> bool:
    return any(PurePosixPath(name).full_match(pattern) for pattern in patterns)


def added_names(base: Path, head: Path, patterns: list[str]) -> frozenset[tuple[str, str]]:
    def names(tree):
        found = set()
        for path in tree.rglob("*.py"):
            name = path.relative_to(tree).as_posix()
            if is_test(name, patterns):
                continue
            # An empty name identifies the module itself, including empty new modules.
            found.add((name, ""))
            try:
                module = ast.parse(path.read_bytes())
            except SyntaxError:
                continue
            for node in module.body:
                if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                    found.add((name, node.name))
        return found
    return frozenset(names(head) - names(base))


def changed_configuration(base: Path, head: Path) -> list[str]:
    """Protected test configuration files that differ between two trees."""
    protected = {"pyproject.toml", "pytest.ini", ".pytest.ini", "setup.cfg", "tox.ini", "noxfile.py"}
    protected |= {str(path.relative_to(tree)) for tree in (base, head)
                  for path in tree.rglob("conftest.py")}
    return sorted(name for name in protected
                  if ((base / name).read_bytes() if (base / name).exists() else None)
                  != ((head / name).read_bytes() if (head / name).exists() else None))


def changed_files(repo: Path, base: str, head: str) -> list[str]:
    """Every file that differs between two commits; both sides of a rename."""
    names = git(repo, "diff", "--name-only", "-z", "--no-renames", "--no-ext-diff", base, head, "--")
    return sorted(name.decode(errors="backslashreplace") for name in names.split(b"\x00") if name)


def changed_protected(repo: Path, base: str, head: str, patterns: tuple[str, ...]) -> list[str]:
    """Files under the repository's protected paths that differ between two commits. Git's
    glob pathspecs match them exactly as the worktree adapter's scope trim does."""
    if not patterns:
        return []
    names = git(repo, "diff", "--name-only", "-z", "--no-renames", "--no-ext-diff", base, head, "--",
                *(f":(top,glob){pattern}" for pattern in patterns))
    return sorted(name.decode(errors="backslashreplace") for name in names.split(b"\x00") if name)


def own_base(repo: Path, base: str, head: str) -> str:
    """The commit head descends from: the base itself, or where it forked from base."""
    try:
        return git(repo, "merge-base", base, head).decode().strip()
    except subprocess.CalledProcessError:
        return base


def test_function_source(source: str, node_id: str) -> str | None:
    """Find a function or class method without importing repository code."""
    _, names = node_id.split("::", 1)
    names = names.split("[", 1)[0].split("::")
    try:
        body = ast.parse(source).body
    except (SyntaxError, ValueError):
        return None
    for index, name in enumerate(names):
        matches = [node for node in body
                   if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef))
                   and node.name == name]
        if len(matches) != 1:
            return None
        node = matches[0]
        if index < len(names) - 1:
            if not isinstance(node, ast.ClassDef):
                return None
            body = node.body
    if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
        return None
    start = min([node.lineno, *(item.lineno for item in node.decorator_list)])
    return "".join(source.splitlines(keepends=True)[start - 1:node.end_lineno])


def changed_existing_tests(base: Path, head: Path, tests: tuple[str, ...]) -> set[str]:
    changed = set()
    for test in tests:
        path = test.split("::", 1)[0]
        def read(tree):
            try:
                source = (tree / path).read_text()
                return test_function_source(source, test) if "::" in test else source
            except (OSError, UnicodeError):
                return None
        before = read(base)
        if before is not None and before != read(head):
            changed.add(test)
    return changed
