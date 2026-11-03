"""Existing tests whose imports reach a change (#359). Static analysis only: no repository code runs."""
import ast
from collections.abc import Iterator
from pathlib import Path, PurePosixPath

# A pathological file can exhaust the parser; it is then skipped, never fatal.
UNREADABLE = (SyntaxError, ValueError, RecursionError, MemoryError)


def node_ids(tree: Path, name: str) -> list[str]:
    """Function and method selectors in source order, so pytest expands parametrized cases itself."""
    nodes = []
    module = ast.parse((tree / name).read_bytes())
    cases = {f"{alias.asname or alias.name}.TestCase" for node in module.body
             if isinstance(node, ast.Import) for alias in node.names if alias.name == "unittest"}
    cases.update(alias.asname or alias.name for node in module.body
                 if isinstance(node, ast.ImportFrom) and node.module == "unittest"
                 for alias in node.names if alias.name == "TestCase")
    disabled = {target.value.id for statement in module.body
                if isinstance(statement, ast.Assign) and isinstance(statement.value, ast.Constant)
                and statement.value.value is False for target in statement.targets
                if isinstance(target, ast.Attribute) and target.attr == "__test__"
                and isinstance(target.value, ast.Name)}
    class_tests = {}
    for node in module.body:
        if getattr(node, "name", None) in disabled:
            continue
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name.startswith("test_"):
            nodes.append(f"{name}::{node.name}")
        elif isinstance(node, ast.ClassDef):
            if any(ast.unparse(base) in cases for base in node.bases):
                cases.add(node.name)
            methods = dict.fromkeys(method for base in node.bases
                                    for method in class_tests.get(ast.unparse(base), ()))
            methods.update(dict.fromkeys(method.name for method in node.body
                           if isinstance(method, (ast.FunctionDef, ast.AsyncFunctionDef))
                           and method.name.startswith("test_")))
            class_tests[node.name] = methods
            if node.name.startswith("Test") or node.name in cases:
                nodes.extend(f"{name}::{node.name}::{method}" for method in methods)
    return nodes


def module_names(path: str, beside_tests: bool) -> set[str]:
    """Names an import may use for a file: its dotted path, without a leading `src`, and,
    for files among the tests, the bare name pytest's rootdir insertion makes importable."""
    parts = PurePosixPath(path).with_suffix("").parts
    if parts[-1] == "__init__":
        parts = parts[:-1]
    if not parts:
        return set()
    names = {".".join(parts)}
    if parts[0] == "src" and len(parts) > 1:
        names.add(".".join(parts[1:]))
    if beside_tests:
        names.add(parts[-1])
    return names


def imported_names(path: str, source: bytes) -> set[str]:
    """Every module name an import in this file could load, including parent packages."""
    try:
        module = ast.parse(source)
    except UNREADABLE:
        return set()
    package = PurePosixPath(path).parent.parts
    names = set()

    def add(dotted):
        parts = dotted.split(".")
        names.update(".".join(parts[:end]) for end in range(1, len(parts) + 1))

    for node in ast.walk(module):
        if isinstance(node, ast.Import):
            for alias in node.names:
                add(alias.name)
        elif isinstance(node, ast.ImportFrom):
            if node.level:
                anchor = package[:len(package) - node.level + 1] if node.level <= len(package) + 1 else ()
                stem = ".".join([*anchor, *([node.module] if node.module else [])])
            else:
                stem = node.module or ""
            if not stem:
                continue
            add(stem)
            for alias in node.names:
                if alias.name != "*":
                    add(f"{stem}.{alias.name}")
    return names


def path_literals(source: bytes) -> set[str]:
    """Strings a file could use as a path: its string constants, plus the consecutive string
    operands of each `/` chain and of each call, joined with `/`, as in
    `Path(__file__).parent / "fixtures" / "x.json"` or `os.path.join("fixtures", "x.json")`."""
    try:
        module = ast.parse(source)
    except UNREADABLE:
        return set()
    found, inner = set(), set()

    def joined(operands):
        run = []
        for operand in [*operands, None]:
            if isinstance(operand, ast.Constant) and isinstance(operand.value, str):
                run.append(operand.value)
                continue
            if len(run) > 1:
                found.add("/".join(part.strip("/") for part in run))
            run = []

    for node in ast.walk(module):
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            found.add(node.value)
        elif isinstance(node, ast.BinOp) and isinstance(node.op, ast.Div) and id(node) not in inner:
            operands = []
            while isinstance(node, ast.BinOp) and isinstance(node.op, ast.Div):
                inner.add(id(node))
                operands.append(node.right)
                node = node.left
            joined([node, *reversed(operands)])
        elif isinstance(node, ast.Call):
            joined(node.args)
    return found


def names_path(literal: str, path: str) -> bool:
    """Whether a literal names a repository-relative path: the path itself, the path under
    some prefix, or a suffix of the path of at least two components."""
    literal = literal.removeprefix("./")
    return (literal == path or literal.endswith("/" + path)
            or "/" in literal and path.endswith("/" + literal))


def select(tree: Path, changed: list[str], is_test, *, modules_only: bool = False) -> Iterator[str]:
    """Existing test selectors in `tree` whose files reach a changed Python file through
    imports, closest first: by import distance, then path, then source order. Lazy: each
    import level is read, and each test file expanded, only when the caller asks for more."""
    files = {path.relative_to(tree).as_posix() for path in tree.rglob("*.py")
             if path.is_file() and not any(part.startswith(".") for part in path.relative_to(tree).parts)}
    sources, imports = {}, {}

    def source(name):
        if name not in sources:
            sources[name] = (tree / name).read_bytes()
        return sources[name]

    # Distance 0: the changed Python files, and every file that names a changed file of any
    # type by a path literal, such as a test hashing a fixture it reads by path (#424).
    frontier = {name for name in changed if name.endswith(".py")}
    basenames = {PurePosixPath(name).name.encode() for name in changed}
    for name in sorted(files - frontier):
        text = source(name)
        if not any(basename in text for basename in basenames):
            continue
        if any(names_path(literal, path) for literal in path_literals(text) for path in changed):
            frontier.add(name)
    frontier = sorted(frontier)
    seen = set(frontier)
    while frontier:
        for name in frontier:
            if (name in files and is_test(name)
                    and (PurePosixPath(name).name.startswith("test_") or name.endswith("_test.py"))):
                try:
                    if modules_only:
                        ast.parse(source(name))
                        yield name
                    else:
                        yield from node_ids(tree, name)
                except UNREADABLE:
                    continue
        modules = set().union(*(module_names(name, is_test(name)) for name in frontier))
        # Only a file naming a module's last part, or importing relatively, can import it.
        tokens = {module.rsplit(".", 1)[-1].encode() for module in modules}
        reached = []
        for name in sorted(files - seen):
            text = source(name)
            if not (b"from ." in text or any(token in text for token in tokens)):
                continue
            if name not in imports:
                imports[name] = imported_names(name, text)
            if imports[name] & modules:
                reached.append(name)
        seen.update(reached)
        frontier = reached
