import ast
import hashlib
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
    if set(config) - {"command", "test-files", "selection-groups"} or not {"command", "test-files"} <= set(config):
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
    groups = config.get("selection-groups", [])
    if not isinstance(groups, list):
        raise ValueError("selection-groups must list complete module groups")
    grouped = set()
    for group in groups:
        if not isinstance(group, list) or not group:
            raise ValueError("each selection group must list complete test modules")
        for name in group:
            if not isinstance(name, str):
                raise ValueError("selection groups must name relative test modules")
            path = PurePosixPath(name)
            if (name != path.as_posix() or path.is_absolute() or ".." in path.parts
                    or any(char in name for char in "*?[]\\:") or path.name == "conftest.py"
                    or path.suffix != ".py" or not is_test(name, files) or not (tree / name).is_file()):
                raise ValueError(f"selection group needs an existing configured test module: {name}")
            if name in grouped:
                raise ValueError(f"selection groups must not overlap or repeat modules: {name}")
            grouped.add(name)
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


def tree_sha256(tree: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(path for path in tree.rglob("*") if path.is_file()):
        name = path.relative_to(tree).as_posix().encode()
        content = path.read_bytes()
        for value in (name, str(path.stat().st_mode & 0o777).encode(), content):
            digest.update(len(value).to_bytes(8, "big"))
            digest.update(value)
    return digest.hexdigest()


def bound_names(source: str) -> set[str]:
    """Conservative static bindings, including aliases and compound statements."""
    module = ast.parse(source)
    names = {node.id for node in ast.walk(module)
             if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Store)}
    names.update(node.name for node in ast.walk(module)
                 if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)))
    names.update(node.asname or node.name.split(".")[0] for node in ast.walk(module)
                 if isinstance(node, ast.alias))
    return names


HELPER_SETUP_FIXTURES = frozenset({'tmp_path', 'tmp_path_factory'})


def validate_helper_setup(source: str, evidence):
    """Resolve bounded fixture dependencies without importing served code."""
    module = ast.parse(source)
    identities = {}
    for name in evidence.setup_functions:
        matches = [node for node in module.body if getattr(node, 'name', None) == name]
        if len(matches) == 1 and isinstance(matches[0], ast.ClassDef):
            raise ValueError(f"Unsupported helper setup {name}: class-based worlds are unsupported; "
                             "select an existing module-level factory")
        if len(matches) == 1 and isinstance(matches[0], ast.AsyncFunctionDef):
            raise ValueError(f"Unsupported helper setup {name}: needs a synchronous function")
        if (len(matches) != 1 or not isinstance(matches[0], ast.FunctionDef)
                or name == evidence.target.split("::")[1]):
            raise ValueError(f"Unsupported helper setup {name}: needs one existing local function distinct from target")
        factory = matches[0]
        args = factory.args
        if factory.decorator_list:
            raise ValueError(f"Unsupported helper setup {name}: decorated functions are unsupported")
        if args.vararg or args.kwarg:
            raise ValueError(f"Unsupported helper setup {name}: variadic parameters are unsupported")
        if any(isinstance(node, (ast.Yield, ast.YieldFrom)) for node in ast.walk(factory)):
            raise ValueError(f"Unsupported helper setup {name}: generator functions are unsupported")
        positional = [*args.posonlyargs, *args.args]
        required = [arg.arg for arg in positional[:len(positional) - len(args.defaults)]]
        required.extend(arg.arg for arg, default in zip(args.kwonlyargs, args.kw_defaults) if default is None)
        unsupported = sorted(set(required) - HELPER_SETUP_FIXTURES)
        if unsupported:
            raise ValueError(f"Unsupported helper setup {name}: required parameters {', '.join(unsupported)} "
                             "are not TL3 fixtures; supported fixtures: tmp_path, tmp_path_factory")
        for node in module.body:
            if node is factory:
                continue
            if name in bound_names(ast.unparse(node)):
                raise ValueError(f"Unsupported helper setup {name}: the function has another binding")
        fixtures = [arg.arg for arg in [*positional, *args.kwonlyargs] if arg.arg in HELPER_SETUP_FIXTURES]
        factory_source = test_function_source(source, f"setup.py::{name}")
        identities[name] = {"fixtures": fixtures,
                            "source_sha256": hashlib.sha256(factory_source.encode()).hexdigest()}
    return identities


def helper_test_edits(base: Path, head: Path, destination: Path, patterns: list[str], evidence,
                      observers: tuple[str, ...]) -> dict:
    """Append new observers to base modules, leaving every old definition intact.

    The supported target is one existing top-level function with an unchanged
    signature and imports. All other test edits must be the named new observers.
    The complete module comparison rejects shadow definitions and dependencies
    changed outside the declared target; no served code is imported here.
    """
    def reject(reason):
        raise ValueError(f"Invalid helper evidence: {reason}")

    target_path, target_name = evidence.target.split("::")
    allowed = {evidence.target, *observers}
    if len(allowed) != len(observers) + 1:
        reject("target and observers must be distinct")
    for observer in observers:
        parts = observer.split("::")
        if len(parts) != 2 or not parts[1].isidentifier() or not parts[1].startswith("test_"):
            reject("observers must be new, top-level, unparameterized tests")
    paths = {path.relative_to(tree).as_posix() for tree in (base, head)
             for path in tree.rglob("*.py") if is_test(path.relative_to(tree).as_posix(), patterns)}
    if target_path not in paths or not is_test(target_path, patterns):
        reject("target must be inside a configured test file")
    sources, definitions = {}, {}
    for name in sorted(paths):
        sides = []
        for label, tree in (("base", base), ("head", head)):
            path = tree / name
            try:
                source = path.read_text() if path.exists() else ""
                module = ast.parse(source)
            except (SyntaxError, UnicodeError) as error:
                reject(f"cannot parse {label} {name}: {type(error).__name__}")
            sources[label, name] = source
            if label == "base":
                bound = bound_names(source)
                for observer in observers:
                    path, function = observer.split("::")
                    if path == name and (function in bound or "*" in bound):
                        reject(f"observer is already bound on base: {observer}")
            remaining = []
            for node in module.body:
                key = f"{name}::{getattr(node, 'name', '')}"
                if key in allowed:
                    if not isinstance(node, ast.FunctionDef) or node.decorator_list:
                        reject(f"{key} must be an undecorated function")
                    if (label, key) in definitions:
                        reject(f"ambiguous definition: {key}")
                    definitions[label, key] = node
                else:
                    remaining.append(node)
            sides.append(ast.dump(ast.Module(body=remaining, type_ignores=[])))
        if sides[0] != sides[1]:
            reject(f"undeclared test code or import changes in {name}")
    old = definitions.get(("base", evidence.target))
    new = definitions.get(("head", evidence.target))
    if old is None or new is None:
        reject("target must exist once on base and head")
    setup_functions = validate_helper_setup(sources["base", target_path], evidence)
    if ast.dump(old) == ast.dump(new):
        reject("target must change")
    for attribute in ("args", "returns", "type_params", "type_comment"):
        def shape(node):
            value = getattr(node, attribute, None)
            return ast.dump(value) if isinstance(value, ast.AST) else repr(value)
        if shape(old) != shape(new):
            reject("target signature must stay unchanged")
    receipts = {}
    for observer in observers:
        if ("base", observer) in definitions or ("head", observer) not in definitions:
            reject(f"observer must be new on head: {observer}")
        name = observer.split("::")[0]
        source = test_function_source(sources["head", name], observer)
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        with target.open("a") as stream:
            stream.write("\n" + source)
        receipts[observer] = hashlib.sha256(source.encode()).hexdigest()
    def target_hash(label):
        source = test_function_source(sources[label, target_path], evidence.target)
        return hashlib.sha256(source.encode()).hexdigest()
    return {"target": evidence.target, "setup_functions": setup_functions, "base_target_sha256": target_hash("base"),
            "head_target_sha256": target_hash("head"), "observers": receipts,
            "base_tree_sha256": tree_sha256(destination), "head_tree_sha256": tree_sha256(head)}
