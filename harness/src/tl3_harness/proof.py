"""The source partition shared by rehearsal, development tests, and Checking."""
import ast
import builtins
import configparser
import hashlib
from pathlib import Path, PurePosixPath
import posixpath
import shutil
import symtable
import textwrap
import tomllib

from .model import Kind, ProofRefusal
from .rules import Violation
from .trees import bound_names, is_test, tree_sha256
from .proof_rules import (
    LOADER_STDLIB, NAMESPACE_API_NAMES, NAMESPACE_API_ATTRIBUTES, NAMESPACE_API_MODULES,
    PURE_SUPPORT_BUILTINS, REFLECTIVE_SUPPORT_ATTRIBUTES, SOURCE_DEFINITION_COUNT,
)


RULE = 'subject-observer-v1'


def source_unit(source, identity, *, optional=False):
    """Find a unique definition or binding without importing repository code."""
    parts = identity.split('::')[1:]
    if not parts:
        return source, None
    body = ast.parse(source).body
    node = None
    for name in parts:
        matches = [item for item in body if name in bound_names(ast.unparse(item))
                   and (getattr(item, 'name', None) == name
                        or isinstance(item, (ast.Assign, ast.AnnAssign)))]
        if not matches and optional:
            return None, None
        if len(matches) != SOURCE_DEFINITION_COUNT:
            raise ProofRefusal('proof.source.definition', module=identity.split('::')[0], symbol=identity)
        node = matches[0]
        body = getattr(node, 'body', [])
    if node is None:
        return source, None
    start = min([node.lineno, *(item.lineno for item in getattr(node, 'decorator_list', []))])
    return ''.join(source.splitlines(keepends=True)[start - 1:node.end_lineno]), (start - 1, node.end_lineno)


def digest(value):
    return hashlib.sha256(value.encode() if isinstance(value, str) else value).hexdigest() if value is not None else None


def unit(tree, identity):
    path = tree / identity.split('::')[0]
    if not path.is_file():
        return None
    return source_unit(path.read_bytes().decode(), identity, optional=True)[0]


def global_names(source, filename):
    table = symtable.symtable(textwrap.dedent(source), filename, 'exec')
    names = set()
    def visit(scope):
        names.update(symbol.get_name() for symbol in scope.get_symbols()
                     if symbol.is_referenced() and symbol.is_global())
        for child in scope.get_children():
            visit(child)
    visit(table)
    return names - set(dir(builtins)) - {'__file__', '__name__', '__package__', '__doc__', '__annotations__'}


def overlaps(left, right):
    return left == right or left.startswith(right + '::') or right.startswith(left + '::')


def written_bindings(source, namespace=()):
    """Static writes in module/class namespaces, including explicit global writes."""
    writes = set()
    def visit(table, scope, function=False):
        for symbol in table.get_symbols():
            if symbol.is_assigned() or symbol.is_imported():
                if symbol.is_declared_global():
                    writes.add((symbol.get_name(),))
                elif not function:
                    writes.add((*scope, symbol.get_name()))
        for child in table.get_children():
            is_class = child.get_type() == 'class' and not function
            visit(child, (*scope, child.get_name()) if is_class else scope, not is_class)
    visit(symtable.symtable(textwrap.dedent(source), '<proof>', 'exec'), namespace)
    return writes


def binding_names(source):
    """All lexical bindings, including captures, deletions and nested shadows.

    #842/#645: loader trust cannot depend on an incomplete AST target list.
    This intentionally treats local shadows and scope declarations as conflicts.
    """
    names = set()
    def visit(table):
        names.update(symbol.get_name() for symbol in table.get_symbols()
                     if symbol.is_assigned() or symbol.is_imported() or symbol.is_parameter()
                     or symbol.is_declared_global() or symbol.is_nonlocal() or symbol.is_type_parameter())
        for child in table.get_children():
            visit(child)
    visit(symtable.symtable(textwrap.dedent(source), '<proof>', 'exec'))
    return names


def binding_statements(tree, name):
    return [statement for statement in tree.body if name in binding_names(ast.unparse(statement))]


def sole_import(tree, name, source):
    statements = binding_statements(tree, name)
    return (len(statements) == 1
            and ast.dump(statements[0]) == ast.dump(ast.parse(source).body[0]))


def validate_partial_definition_uses(tree, module, name):
    """Every remaining reference to a partially copied definition is a direct call."""
    parents = {id(child): node for node in ast.walk(tree) for child in ast.iter_child_nodes(node)}
    for node in ast.walk(tree):
        if isinstance(node, (ast.Attribute, ast.Subscript)) and isinstance(node.ctx, (ast.Store, ast.Del)):
            root = node
            while isinstance(root, (ast.Attribute, ast.Subscript)):
                root = root.value
            if isinstance(root, ast.Name) and root.id == name:
                raise ProofRefusal('proof.support.definition_write', module=module, symbol=name)
        if isinstance(node, ast.Name) and node.id == name:
            parent = parents.get(id(node))
            if not (isinstance(parent, ast.Call) and parent.func is node):
                raise ProofRefusal('proof.support.definition_use', module=module, symbol=name)


def validate_partial_literal(source, module_source, module, symbol):
    """Only immutable, singly bound data can be copied without its context."""
    statements = ast.parse(textwrap.dedent(source)).body
    for statement in statements:
        if isinstance(statement, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            class RemoveDefinition(ast.NodeTransformer):
                removed = False
                def visit(self, node):
                    if not self.removed and ast.dump(node) == ast.dump(statement):
                        self.removed = True
                        return ast.copy_location(ast.Pass(), node)
                    return super().visit(node)
            remover = RemoveDefinition()
            stripped = remover.visit(ast.parse(module_source))
            if not remover.removed or statement.name in binding_names(ast.unparse(stripped)):
                raise ProofRefusal('proof.support.definition_binding', module=module, symbol=statement.name)
            # #842 r3: only the definition is copied, so the rest of the module may
            # only call it. Aliases, attribute access and arguments could mutate the
            # object (for example __defaults__) in GREEN but not in RED.
            validate_partial_definition_uses(stripped, module, statement.name)
            validate_namespace_apis(ast.parse(module_source), module)
        if not isinstance(statement, (ast.Assign, ast.AnnAssign)):
            continue
        try:
            value = ast.literal_eval(statement.value)
        except (ValueError, TypeError, SyntaxError):
            raise ProofRefusal('proof.support.literal_initializer', module=module, symbol=symbol) from None
        if not (len(statements) == 1 and isinstance(statement, ast.Assign)
                and len(statement.targets) == 1 and isinstance(statement.targets[0], ast.Name)):
            raise ProofRefusal('proof.support.literal_assignment', module=module, symbol=symbol)
        binding = statement.targets[0].id
        class RemoveLiteral(ast.NodeTransformer):
            removed = False
            def visit_Assign(self, node):
                if not self.removed and ast.dump(node) == ast.dump(statement):
                    self.removed = True
                    return ast.copy_location(ast.Pass(), node)
                return self.generic_visit(node)
        remover = RemoveLiteral()
        stripped = remover.visit(ast.parse(module_source))
        try:
            # literal_eval yields only builtin values: hashability excludes every
            # mutable container, including one nested in an immutable tuple.
            hash(value)
            # Erasing only the initializer lets the compiler find every other
            # binding, even inside the same class or compound statement.
            safe = remover.removed and binding not in binding_names(ast.unparse(stripped))
        except (TypeError, SyntaxError):
            safe = False
        if not safe:
            raise ProofRefusal('proof.support.literal_binding', module=module, symbol=binding)
        # A dynamic namespace write elsewhere could change even an immutable
        # binding, without appearing as a second write in the symbol table.
        validate_namespace_apis(ast.parse(module_source), module)


def observer_context_imports(tree, module, subjects):
    """Recognize the isolated script loader and CLI setup from tessera #1638."""
    allowed = set()
    plain_path = (sole_import(tree, 'Path', 'from pathlib import Path')
                  and not binding_statements(tree, '__file__'))
    for function in (node for node in tree.body if isinstance(node, ast.FunctionDef)):
        for subject in {item.split('::')[0] for item in subjects} if plain_path else ():
            template = ast.parse(f'''def {function.name}():
    import importlib.util
    path = Path(__file__).resolve().parents[{len(Path(module).parts) - 1}] / {subject!r}
    spec = importlib.util.spec_from_file_location("proof_subject", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module
''').body[0]
            # The name given to a fresh, unregistered module has no proof meaning.
            if len(function.body) == 6 and isinstance(function.body[2], ast.Assign):
                call = function.body[2].value
                if isinstance(call, ast.Call) and call.args and isinstance(call.args[0], ast.Constant):
                    template.body[2].value.args[0] = call.args[0]
            if (ast.dump(function) == ast.dump(template)
                    and binding_statements(tree, function.name) == [function]):
                allowed.update(id(node) for node in ast.walk(function))
    parents = {id(child): node for node in ast.walk(tree) for child in ast.iter_child_nodes(node)}
    plain_sys = sole_import(tree, 'sys', 'import sys')
    mock_roots = set()
    if sole_import(tree, 'mock', 'from unittest import mock'):
        mock_roots.add('mock')
    if sole_import(tree, 'unittest', 'import unittest.mock'):
        mock_roots.add('unittest.mock')
    canonical_imports = {ast.dump(ast.parse(source).body[0]) for source in (
        'import unittest', 'import unittest.mock', 'from unittest import mock')}
    if any(isinstance(node, (ast.Import, ast.ImportFrom))
           and any(name.split('.')[0] == 'unittest' for name in (
               [node.module or ''] if isinstance(node, ast.ImportFrom) else [alias.name for alias in node.names]))
           and ast.dump(node) not in canonical_imports for node in ast.walk(tree)):
        mock_roots.clear()
    # A real import is insufficient if its patch implementation can be replaced
    # or aliased through another use of the imported object.
    for node in ast.walk(tree):
        if not isinstance(node, ast.Name) or node.id not in {'mock', 'unittest'}:
            continue
        root = node
        while isinstance(parents.get(id(root)), ast.Attribute):
            root = parents[id(root)]
        expression = ast.unparse(root)
        call = parents.get(id(root))
        if (expression in {name + '.patch.object' for name in mock_roots}
                and isinstance(call, ast.Call) and call.func is root
                and len(call.args) == 3 and not call.keywords
                and isinstance(call.args[1], ast.Constant)
                and isinstance(call.args[0], ast.Name)
                and plain_sys and call.args[0].id == 'sys' and call.args[1].value == 'argv'
                and isinstance(parents.get(id(call)), ast.withitem)):
            continue
        mock_roots.clear()
        break
    for node in ast.walk(tree):
        if not isinstance(node, ast.Name) or node.id != 'sys':
            continue
        parent = parents[id(node)]
        if isinstance(parent, ast.Attribute) and parent.attr == 'executable' and isinstance(parent.ctx, ast.Load):
            continue
        # Only a scoped argv patch is CLI setup. sys cannot escape as an object.
        if (isinstance(parent, ast.Call) and ast.unparse(parent.func) in {name + '.patch.object' for name in mock_roots}
                and len(parent.args) == 3 and parent.args[0] is node
                and isinstance(parent.args[1], ast.Constant) and parent.args[1].value == 'argv'
                and plain_sys and not parent.keywords and isinstance(parents.get(id(parent)), ast.withitem)):
            allowed.add(id(parent))
            continue
        break
    else:
        allowed.update(id(node) for node in ast.walk(tree) if plain_sys and isinstance(node, ast.Import)
                       and len(node.names) == 1 and node.names[0].name == 'sys' and not node.names[0].asname)
    return allowed


def validate_namespace_apis(tree, module, allowed=()):
    """Reject indirect writes as well as syntactic binding targets."""
    violations = []
    names, attributes, modules = NAMESPACE_API_NAMES, NAMESPACE_API_ATTRIBUTES, NAMESPACE_API_MODULES
    nodes = list(ast.walk(tree))
    for node in nodes:
        if isinstance(node, ast.ImportFrom) and any(alias.name == '*' for alias in node.names):
            violations.extend(ProofRefusal('proof.support.wildcard_import', module=module, symbol=node.module or '.').violations)
        symbol = None
        if isinstance(node, ast.Name) and node.id in names:
            symbol = node.id
        elif isinstance(node, ast.Attribute) and node.attr in attributes:
            symbol = node.attr
        elif isinstance(node, ast.ImportFrom) and node.module in {'operator', '_operator'}:
            symbol = next((alias.name for alias in node.names if alias.name in names), None)
        elif isinstance(node, (ast.Import, ast.ImportFrom)) and id(node) not in allowed:
            imported = [node.module or ''] if isinstance(node, ast.ImportFrom) else [alias.name for alias in node.names]
            symbol = next((name for name in imported if name.split('.')[0] in modules), None)
        if symbol:
            violations.extend(ProofRefusal('proof.support.namespace_api', module=module, symbol=symbol).violations)
    if violations:
        raise ProofRefusal.from_violations(violations)


def validate_support_calls(tree, module, allowed, context):
    """#645: support cannot invoke imported objects, aliases or field evaluators."""
    violations = []
    pure = set(PURE_SUPPORT_BUILTINS) - set().union(*(binding_names(source) for source in context))
    functions = {node.name for node in tree.body if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
                 and binding_statements(tree, node.name) == [node]
                 and node.name not in binding_names(ast.unparse(ast.Module(body=node.body, type_ignores=[])))
                 and node.name not in {arg.arg for arg in ast.walk(node.args) if isinstance(arg, ast.arg)}}
    def pure_value(node):
        # Even sorted(key=...) and int(obj) can call arbitrary methods. Keep
        # builtin inputs to literal data and results of the same pure calls.
        try:
            ast.literal_eval(node)
            return not any(isinstance(child, ast.Call) for child in ast.walk(node))
        except (ValueError, TypeError, SyntaxError):
            return (isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
                    and node.func.id in pure and not node.keywords
                    and all(pure_value(arg) for arg in node.args))
    for node in ast.walk(tree):
        if id(node) in allowed:
            continue
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)) and node.decorator_list:
            violations.extend(ProofRefusal('proof.support.decorator', module=module, symbol=node.name).violations)
        if isinstance(node, ast.ClassDef) and (node.bases or node.keywords):
            violations.extend(ProofRefusal('proof.support.class_bases', module=module, symbol=node.name).violations)
        if isinstance(node, (ast.With, ast.AsyncWith)) and any(id(item.context_expr) not in allowed for item in node.items):
            violations.extend(ProofRefusal('proof.support.context_manager', module=module).violations)
        if isinstance(node, ast.Call):
            if isinstance(node.func, ast.Name) and (node.func.id in functions or pure_value(node)):
                continue
            callee = ast.unparse(node.func)
            violations.extend(ProofRefusal('proof.support.call_allowlist', module=module, symbol=callee, bad_value=callee).violations)
        # Percent templates can dispatch to arbitrary mapping/operand methods.
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Mod):
            try:
                ast.literal_eval(node.left)
                ast.literal_eval(node.right)
                safe = isinstance(node.left, ast.Constant) and isinstance(node.left.value, (str, bytes))
            except (ValueError, TypeError, SyntaxError):
                safe = False
            if not safe or '__' in str(node.left.value):
                violations.extend(ProofRefusal('proof.support.percent_format', module=module).violations)
        if isinstance(node, ast.Attribute) and node.attr in REFLECTIVE_SUPPORT_ATTRIBUTES:
            violations.extend(ProofRefusal('proof.support.reflective_attribute', module=module, symbol=node.attr).violations)
    if violations:
        raise ProofRefusal.from_violations(violations)


def validate_support(source, module, *, observer_context=False, subjects=(), context=()):
    """Report independent support failures together, without changing acceptance."""
    violations = []
    try:
        tree = ast.parse(textwrap.dedent(source))
        allowed = observer_context_imports(tree, module, subjects) if observer_context else set()
        for validate, args in ((validate_namespace_apis, (tree, module, allowed)),
                               (validate_support_calls, (tree, module, allowed, context or (source,)))):
            try:
                validate(*args)
            except ProofRefusal as error:
                violations.extend(error.violations)
        for node in ast.walk(tree):
            if isinstance(node, (ast.Attribute, ast.Subscript)) and isinstance(node.ctx, (ast.Store, ast.Del)):
                root = node
                while isinstance(root, (ast.Attribute, ast.Subscript)):
                    root = root.value
                symbol = root.id if isinstance(root, ast.Name) else ast.unparse(root)
                violations.extend(ProofRefusal('proof.support.member_write', module=module, symbol=symbol).violations)
    except SyntaxError as error:
        if violations:
            raise ProofRefusal.from_violations(violations) from None
        raise ProofRefusal('proof.source.unparsable', module=module, bad_value=error.msg) from None
    if violations:
        raise ProofRefusal.from_violations(violations)


def conftest_subjects(base, head, plan):
    """Only explicitly partitioned fixture source escapes the configuration guard."""
    allowed = set()
    subjects = [unit for check in plan if check.proof for unit in check.proof.subjects]
    for name in {unit.split('::')[0] for unit in subjects if Path(unit.split('::')[0]).name == 'conftest.py'}:
        def remainder(tree):
            path = tree / name
            source = path.read_text() if path.is_file() else ''
            spans = [source_unit(source, unit, optional=True)[1] for unit in subjects if unit.split('::')[0] == name]
            if name in subjects:
                return ''
            lines = source.splitlines(keepends=True)
            for start, end in sorted({span for span in spans if span}, reverse=True):
                lines[start:end] = []
            return ast.dump(ast.parse(''.join(lines)))
        if remainder(base) == remainder(head):
            allowed.add(name)
    return allowed


PYTEST_INI = (('pytest.ini', 'pytest'), ('.pytest.ini', 'pytest'), ('tox.ini', 'pytest'), ('setup.cfg', 'tool:pytest'))


def pytest_pythonpath(tree: Path):
    """Directories a tree's pytest configuration puts on sys.path (its `pythonpath` option)."""
    entries = []
    for name, keys in (('pyproject.toml', ('tool', 'pytest', 'ini_options')), ('pyproject.toml', ('tool', 'pytest')),
                       ('pytest.toml', ('pytest',)), ('.pytest.toml', ('pytest',))):
        try:
            table = tomllib.loads((tree / name).read_text()) if (tree / name).is_file() else {}
            for key in keys:
                table = table.get(key, {}) if isinstance(table, dict) else {}
            value = table.get('pythonpath', []) if isinstance(table, dict) else []
        except (tomllib.TOMLDecodeError, UnicodeError):
            continue
        if isinstance(value, str):
            entries += value.split()
        elif isinstance(value, list):
            entries += [item for item in value if isinstance(item, str)]
    for name, section in PYTEST_INI:
        parser = configparser.ConfigParser(interpolation=None)
        try:
            parser.read_string((tree / name).read_text() if (tree / name).is_file() else '')
        except (configparser.Error, UnicodeError):
            continue
        entries += parser.get(section, 'pythonpath', fallback='').split()
    return {posixpath.normpath(entry) for entry in entries}


def stdlib_shadow(tree: Path, modules=LOADER_STDLIB):
    """First file importable under one of `modules` as a top-level name (#876).

    An import root is the repository root, a pytest `pythonpath` entry, or a
    directory without `__init__.py`: pytest's rootdir insertion (`--rootdir=.`)
    puts the nearest such ancestor of a test file on sys.path. A file inside a
    regular package imports under the package's dotted name, so
    `_pytest/pathlib.py` is `_pytest.pathlib`, never `pathlib`."""
    configured = pytest_pythonpath(tree)
    for path in sorted(tree.rglob('*.py')):
        relative = path.relative_to(tree)
        for depth, part in enumerate(relative.parts):
            stem = part[:-3] if depth == len(relative.parts) - 1 else part
            parent = PurePosixPath(*relative.parts[:depth])
            root = (depth == 0 or parent.as_posix() in configured
                    or not (tree / parent / '__init__.py').is_file())
            if stem in modules and root:
                return stem, relative.as_posix()
    return None


def refuse_stdlib_shadow(tree: Path, observer):
    shadow = stdlib_shadow(tree)
    if shadow:
        module, relative = shadow
        raise ProofRefusal('proof.loader.stdlib_shadow', module=observer, symbol=relative, path=relative, stdlib=module)


def assemble_proof(base: Path, head: Path, destination: Path, check, config, *, role='maker'):
    try:
        return _assemble_proof(base, head, destination, check, config)
    except SyntaxError as error:
        module = next((unit.split('::')[0] for unit in check.tests), '')
        refusal = ProofRefusal('proof.source.unparsable', module=module, bad_value=error.msg)
        raise refusal.for_check(check, role) from None
    except ProofRefusal as error:
        raise error.for_check(check, role) from None


def _assemble_proof(base: Path, head: Path, destination: Path, check, config):
    """Base subject plus head observers. No head subject code is ever copied to RED."""
    if destination.exists():
        raise ProofRefusal('proof.tree.destination_fresh')
    shutil.copytree(base, destination)
    declaration = check.proof
    subjects = declaration.subjects if declaration else ()
    support = declaration.observer_support if declaration else ()
    outcome_observer = (check.kind not in (Kind.DEMONSTRATING, Kind.REPRODUCING)
                        or check.declared_symptom is not None or check.forcing is not None and not subjects)
    observers = () if outcome_observer else tuple(dict.fromkeys(test.split('[', 1)[0] for test in check.tests))
    for subject in subjects:
        if any(overlaps(subject, item) for item in (*support, *observers)):
            raise ProofRefusal('proof.subject.overlap', module=subject.split('::')[0], symbol=subject)
        if unit(base, subject) == unit(head, subject):
            raise ProofRefusal('proof.subject.changed', module=subject.split('::')[0], symbol=subject)
    for dependency in support:
        if unit(base, dependency) is not None:
            raise ProofRefusal('proof.support.new', module=dependency.split('::')[0], symbol=dependency)
    for index, observer in enumerate(observers):
        if any(overlaps(observer, other) for other in observers[index + 1:]):
            raise ProofRefusal('proof.observer.overlap', module=observer.split('::')[0], symbol=observer)
    paths = {item.split('::')[0] for item in (*observers, *support)}
    identities, inferred_support, loader_observers = {}, set(), []
    for name in sorted(paths):
        if not is_test(name, config['test-files']) and any(item.split('::')[0] == name for item in observers):
            raise ProofRefusal('proof.observer.test_path', module=name)
        target, old_path, new_path = destination / name, base / name, head / name
        if not new_path.is_file():
            raise ProofRefusal('proof.observer.module_missing', module=name)
        old = old_path.read_bytes().decode() if old_path.is_file() else ''
        new = new_path.read_bytes().decode()
        observer_context = not old_path.exists() and any(item.split('::')[0] == name for item in observers)
        if observer_context:
            # Canonical import spelling cannot authenticate a repository-local
            # replacement of the stdlib inputs used by the loader exception.
            refuse_stdlib_shadow(head, name)
            loader_observers.append(name)
        violations = []
        def collect(validate, *args, field=None, **kwargs):
            try:
                return validate(*args, **kwargs)
            except ProofRefusal as error:
                if any(item.rule.gating for item in error.violations):
                    raise
                violations.extend(Violation(item.rule, {**item.facts, **({'field': field} if field else {})})
                                  for item in error.violations)

        requested = [item for item in (*observers, *support) if item.split('::')[0] == name]
        for statement in ast.parse(new).body:
            if (isinstance(statement, (ast.Import, ast.ImportFrom))
                    and bound_names(ast.unparse(statement)) & {item.split('::')[-1] for item in requested if item in support}):
                collect(validate_support, ast.unparse(statement), name)
        support_required = set()
        for item in requested:
            if item in support:
                text, _ = source_unit(new, item)
                collect(validate_support, text, name, observer_context=observer_context and item == name, subjects=subjects, context=(old, new),
                        field=f'/proof/observer_support/{support.index(item)}')
                if old_path.exists() and '::' in item:
                    collect(validate_partial_literal, text, new, name, item.split('::')[-1],
                            field=f'/proof/observer_support/{support.index(item)}')
                if item != name:
                    support_required.update(global_names(text, name))
        for statement in ast.parse(new).body:
            if (isinstance(statement, (ast.Import, ast.ImportFrom))
                    and bound_names(ast.unparse(statement)) & support_required):
                collect(validate_support, ast.unparse(statement), name)
        if violations:
            raise ProofRefusal.from_violations(violations)
        if not old_path.exists():
            if any(subject.split('::')[0] == name for subject in subjects):
                raise ProofRefusal('proof.observer.subject_module', module=name)
            # A new module has no old subject to overwrite. Its complete source is
            # the observer context, including its imports and pytest fixtures.
            for item in requested:
                identities[item] = digest(source_unit(new, item)[0])
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(new.encode())
            continue
        replacements, grafts, required_texts = [], [], []
        protected = written_bindings(old)
        protected.update(tuple(subject.split('::')[1:]) for subject in subjects if subject.split('::')[0] == name)
        def guard(text, item, namespace=()):
            if item in observers:
                return
            written = written_bindings(text, namespace)
            conflict = written & protected
            if conflict:
                symbol = '::'.join(sorted(conflict)[0])
                raise ProofRefusal('proof.support.graft_binding', module=name, symbol=symbol)
        for item in requested:
            text, _ = source_unit(new, item)
            required_texts.append(text)
            if item in observers and isinstance(ast.parse(textwrap.dedent(text)).body[0], ast.ClassDef):
                raise ProofRefusal('proof.observer.exact_method', module=name, symbol=item.split('::')[-1])
            if '::' not in item:
                raise ProofRefusal('proof.support.existing_module', module=name)
            identities[item] = digest(text)
            try:
                _, span = source_unit(old, item)
            except ProofRefusal:
                span = None
                parts = item.split('::')
                namespace = tuple(parts[1:-1])
                if len(parts) > 2:
                    # A new class carries the method's class and fixture context.
                    parent = '::'.join(parts[:2])
                    try:
                        source_unit(old, parent)
                    except ProofRefusal:
                        if any(overlaps(parent, subject) for subject in subjects):
                            raise ProofRefusal('proof.observer.subject_class', module=name, symbol=parts[1])
                        text, _ = source_unit(new, parent)
                        required_texts.append(text)
                        namespace = ()
                    else:
                        _, parent_span = source_unit(old, parent)
                        guard(text, item, namespace)
                        replacements.append((parent_span[1], parent_span[1], '\n' + text))
                        continue
                elif parts[-1] in bound_names(old) or '*' in bound_names(old):
                    raise ProofRefusal('proof.observer.binding_shadow', module=name, symbol=parts[-1])
            if span:
                guard(text, item, tuple(item.split('::')[1:-1]))
                replacements.append((*span, text))
            elif text not in grafts:
                guard(text, item, namespace)
                grafts.append(text)
        lines = old.splitlines(keepends=True)
        for start, end, text in sorted(replacements, reverse=True):
            lines[start:end] = [text + ('\n' if not text.endswith('\n') else '')]
        composed = ''.join(lines) + ''.join('\n' + text + '\n' for text in grafts)
        required = set().union(*(global_names(text, name) for text in required_texts))
        table = symtable.symtable(composed, name, 'exec')
        bound = {symbol.get_name() for symbol in table.get_symbols() if symbol.is_assigned() or symbol.is_imported()}
        imports = []
        for statement in ast.parse(new).body:
            if not isinstance(statement, (ast.Import, ast.ImportFrom)):
                continue
            aliases = [alias for alias in statement.names
                       if (alias.asname or (alias.name.split('.')[0] if isinstance(statement, ast.Import) else alias.name))
                       in required - bound]
            if aliases:
                statement.names = aliases
                imports.append(ast.unparse(statement))
                bound.update(bound_names(imports[-1]))
        # #842: only immutable, singly bound literal data has no omitted context.
        for symbol in sorted(required - bound):
            item = name + '::' + symbol
            text, _ = source_unit(new, item, optional=True)
            if text is None:
                continue
            statements = ast.parse(textwrap.dedent(text)).body
            if len(statements) != 1:
                continue
            statement = statements[0]
            if not (isinstance(statement, ast.Assign) and len(statement.targets) == 1
                    and isinstance(statement.targets[0], ast.Name) and statement.targets[0].id == symbol):
                continue
            try:
                ast.literal_eval(statement.value)
            except (ValueError, TypeError, SyntaxError):
                continue
            validate_support(text, name)
            guard(text, item)
            if '*' in bound_names(old):
                raise ProofRefusal('proof.support.literal_wildcard', module=name, symbol=symbol)
            validate_partial_literal(text, new, name, symbol)
            imports.append(text)
            bound.add(symbol)
            inferred_support.add(item)
            identities[item] = digest(text)
        missing = required - bound
        if missing:
            raise ProofRefusal('proof.observer.unresolved_symbol', module=name, symbol=sorted(missing)[0])
        # Imports and literal support follow a module docstring and __future__ statements.
        module = ast.parse(composed)
        insertion = 0
        for statement in module.body:
            if (isinstance(statement, ast.Expr) and isinstance(statement.value, ast.Constant)
                    and isinstance(statement.value.value, str)
                    or isinstance(statement, ast.ImportFrom) and statement.module == '__future__'):
                insertion = statement.end_lineno
            else:
                break
        lines = composed.splitlines(keepends=True)
        lines[insertion:insertion] = [line + '\n' for line in imports]
        target.write_bytes(''.join(lines).encode())
    for name in loader_observers[:1]:
        # RED runs on base plus the copied observers: a head-only package marker does not protect it.
        refuse_stdlib_shadow(destination, name)
    for subject in subjects:
        if unit(destination, subject) != unit(base, subject):
            raise ProofRefusal('proof.subject.preserved', module=subject.split('::')[0], symbol=subject)
    inferred = sorted({path.relative_to(tree).as_posix() for tree in (base, head)
                       for path in tree.rglob('*') if path.is_file()
                       and (not (base / path.relative_to(tree)).is_file()
                            or not (head / path.relative_to(tree)).is_file()
                            or (base / path.relative_to(tree)).read_bytes() != (head / path.relative_to(tree)).read_bytes())})
    def identity(tree, name):
        try:
            path = tree / name
            return digest(path.read_bytes() if '::' not in name and path.is_file() else unit(tree, name))
        except (ProofRefusal, SyntaxError, UnicodeError):
            return None
    return {'rule': RULE, 'declaration': declaration.as_json() if declaration else None,
            'mode': 'pytest-outcome' if outcome_observer else 'head-observer',
            'subjects': {name: {'base': identity(base, name), 'head': identity(head, name)}
                         for name in subjects or inferred if name.endswith('.py') or '::' in name},
            'observer_sha256': {test: identity(head, test.split('[', 1)[0]) for test in check.tests},
            'support_sha256': {name: identities[name] for name in sorted(set(support) | inferred_support)},
            'source_base_tree_sha256': tree_sha256(base),
            'base_tree_sha256': tree_sha256(destination), 'head_tree_sha256': tree_sha256(head)}
