"""The source partition shared by rehearsal, development tests, and Checking."""
import ast
import builtins
import hashlib
from pathlib import Path
import shutil
import symtable
import textwrap

from .model import Kind, ProofRefusal
from .trees import bound_names, is_test, tree_sha256


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
        if len(matches) != 1:
            raise ProofRefusal('missing or ambiguous source definition', identity.split('::')[0], name)
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


def validate_partial_literal(source, module_source, module, symbol):
    """Only immutable, singly bound data can be copied without its context."""
    statements = ast.parse(textwrap.dedent(source)).body
    for statement in statements:
        if not isinstance(statement, (ast.Assign, ast.AnnAssign)):
            continue
        try:
            value = ast.literal_eval(statement.value)
        except (ValueError, TypeError, SyntaxError):
            continue
        if not (len(statements) == 1 and isinstance(statement, ast.Assign)
                and len(statement.targets) == 1 and isinstance(statement.targets[0], ast.Name)):
            raise ProofRefusal('partial literal support must bind one plain name in a single assignment; '
                               'put test data inside the observer or a new support factory', module, symbol)
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
            raise ProofRefusal('partial literal support must be immutable and bound exactly once; '
                               'put test data inside the observer or a new support factory', module, binding)
        # A dynamic namespace write elsewhere could change even an immutable
        # binding, without appearing as a second write in the symbol table.
        validate_namespace_apis(ast.parse(module_source), module)


def loader_result_names(tree, loaders):
    """Find names bound only to fresh modules from the verified loaders."""
    candidates = set()
    class RemoveResults(ast.NodeTransformer):
        def visit_Assign(self, node):
            if (len(node.targets) == 1 and isinstance(node.targets[0], ast.Name)
                    and isinstance(node.value, ast.Call) and isinstance(node.value.func, ast.Name)
                    and node.value.func.id in loaders and not node.value.args and not node.value.keywords):
                candidates.add(node.targets[0].id)
                return ast.copy_location(ast.Pass(), node)
            return self.generic_visit(node)
    stripped = RemoveResults().visit(ast.parse(ast.unparse(tree)))
    try:
        return candidates - binding_names(ast.unparse(stripped))
    except SyntaxError:
        # Removing an outer assignment can invalidate a nonlocal declaration.
        # Such a captured module is outside this narrow loader exception.
        return set()


def observer_context_imports(tree, module, subjects):
    """Recognize the isolated script loader and CLI setup from tessera #1638."""
    allowed, loaders = set(), set()
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
                allowed.add(id(function.body[0]))
                loaders.add(function.name)
    parents = {id(child): node for node in ast.walk(tree) for child in ast.iter_child_nodes(node)}
    plain_sys = sole_import(tree, 'sys', 'import sys')
    loaded_modules = loader_result_names(tree, loaders)
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
        if node.id == 'unittest' and expression in {'unittest.TestCase', 'unittest.main'}:
            continue
        call = parents.get(id(root))
        if (expression in {name + '.patch.object' for name in mock_roots}
                and isinstance(call, ast.Call) and call.func is root
                and len(call.args) == 3 and not call.keywords
                and isinstance(call.args[1], ast.Constant)
                and isinstance(call.args[0], ast.Name)
                and (call.args[0].id in loaded_modules
                     or call.args[0].id == 'sys' and call.args[1].value == 'argv')
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
                and not parent.keywords and isinstance(parents.get(id(parent)), ast.withitem)):
            continue
        break
    else:
        allowed.update(id(node) for node in ast.walk(tree) if plain_sys and isinstance(node, ast.Import)
                       and len(node.names) == 1 and node.names[0].name == 'sys' and not node.names[0].asname)
    return allowed


def validate_namespace_apis(tree, module, allowed=()):
    """Reject indirect writes as well as syntactic binding targets."""
    names = {'globals', 'locals', 'vars', 'eval', 'exec', 'compile', '__import__',
             'getattr', 'setattr', 'delattr', '__builtins__', 'attrgetter', 'methodcaller'}
    attributes = {'__dict__', '__globals__', '__builtins__', '__getattribute__',
                  '__setattr__', '__delattr__', 'f_globals', 'f_locals', 'gi_frame',
                  'cr_frame', 'tb_frame', '_getframe', 'currentframe', 'modules', 'attrgetter', 'methodcaller'}
    modules = {'builtins', 'sys', 'inspect', 'importlib', '__main__'}
    nodes = list(ast.walk(tree))
    for node in nodes:
        if isinstance(node, ast.ImportFrom) and any(alias.name == '*' for alias in node.names):
            raise ProofRefusal('wildcard import in support can rebind base bindings', module, node.module or '.')
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
            raise ProofRefusal('dynamic namespace API in support can rebind base subjects', module, symbol)


def validate_support(source, module, *, observer_context=False, subjects=()):
    """Support adds plain bindings, never writes to existing objects or namespaces."""
    tree = ast.parse(textwrap.dedent(source))
    allowed = observer_context_imports(tree, module, subjects) if observer_context else set()
    validate_namespace_apis(tree, module, allowed)
    nodes = list(ast.walk(tree))
    for node in nodes:
        if isinstance(node, (ast.Attribute, ast.Subscript)) and isinstance(node.ctx, (ast.Store, ast.Del)):
            root = node
            while isinstance(root, (ast.Attribute, ast.Subscript)):
                root = root.value
            symbol = root.id if isinstance(root, ast.Name) else ast.unparse(root)
            raise ProofRefusal('support may bind only brand-new plain names, never members or subscripts', module, symbol)


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


def assemble_proof(base: Path, head: Path, destination: Path, check, config):
    """Base subject plus head observers. No head subject code is ever copied to RED."""
    if destination.exists():
        raise ProofRefusal('destination must be a fresh tree')
    shutil.copytree(base, destination)
    declaration = check.proof
    subjects = declaration.subjects if declaration else ()
    support = declaration.observer_support if declaration else ()
    outcome_observer = (check.kind not in (Kind.DEMONSTRATING, Kind.REPRODUCING)
                        or check.declared_symptom is not None or check.forcing is not None and not subjects)
    observers = () if outcome_observer else tuple(dict.fromkeys(test.split('[', 1)[0] for test in check.tests))
    for subject in subjects:
        if any(overlaps(subject, item) for item in (*support, *observers)):
            raise ProofRefusal('subject overlaps an observer or its support', subject.split('::')[0])
        if unit(base, subject) == unit(head, subject):
            raise ProofRefusal('subject must identify changed source', subject.split('::')[0])
    for dependency in support:
        if unit(base, dependency) is not None:
            raise ProofRefusal('observer support must be new and cannot replace a base subject', dependency.split('::')[0])
    for index, observer in enumerate(observers):
        if any(overlaps(observer, other) for other in observers[index + 1:]):
            raise ProofRefusal('overlapping observer selections', observer.split('::')[0])
    paths = {item.split('::')[0] for item in (*observers, *support)}
    identities, inferred_support = {}, set()
    for name in sorted(paths):
        if not is_test(name, config['test-files']) and any(item.split('::')[0] == name for item in observers):
            raise ProofRefusal('observer is outside configured tests', name)
        target, old_path, new_path = destination / name, base / name, head / name
        if not new_path.is_file():
            raise ProofRefusal('missing head observer module', name)
        old = old_path.read_bytes().decode() if old_path.is_file() else ''
        new = new_path.read_bytes().decode()
        observer_context = not old_path.exists() and any(item.split('::')[0] == name for item in observers)
        if observer_context:
            # Canonical import spelling cannot authenticate a repository-local
            # replacement of the stdlib inputs used by the loader exception.
            trusted = {'pathlib', 'unittest', 'importlib'}
            for path in head.rglob('*.py'):
                relative = path.relative_to(head)
                if relative.stem in trusted or trusted.intersection(relative.parts[:-1]):
                    raise ProofRefusal('observer loader requires unshadowed standard-library modules; '
                                       'rename repository-local support', name, relative.as_posix())
        requested = [item for item in (*observers, *support) if item.split('::')[0] == name]
        for statement in ast.parse(new).body:
            if (isinstance(statement, (ast.Import, ast.ImportFrom))
                    and bound_names(ast.unparse(statement)) & {item.split('::')[-1] for item in requested if item in support}):
                validate_support(ast.unparse(statement), name)
        support_required = set()
        for item in requested:
            if item in support:
                text, _ = source_unit(new, item)
                validate_support(text, name, observer_context=observer_context and item == name, subjects=subjects)
                if old_path.exists() and '::' in item:
                    validate_partial_literal(text, new, name, item.split('::')[-1])
                if item != name:
                    support_required.update(global_names(text, name))
        for statement in ast.parse(new).body:
            if (isinstance(statement, (ast.Import, ast.ImportFrom))
                    and bound_names(ast.unparse(statement)) & support_required):
                validate_support(ast.unparse(statement), name)
        if not old_path.exists():
            if any(subject.split('::')[0] == name for subject in subjects):
                raise ProofRefusal('new observer module would carry a declared subject onto base', name)
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
                raise ProofRefusal('support graft would rebind a base or subject binding', name, symbol)
        for item in requested:
            text, _ = source_unit(new, item)
            required_texts.append(text)
            if item in observers and isinstance(ast.parse(textwrap.dedent(text)).body[0], ast.ClassDef):
                raise ProofRefusal('select exact observer methods instead of a whole class', name, item.split('::')[-1])
            if '::' not in item:
                raise ProofRefusal('an existing module cannot be observer support', name)
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
                            raise ProofRefusal('new observer class would carry a declared subject onto base', name, parts[1])
                        text, _ = source_unit(new, parent)
                        required_texts.append(text)
                        namespace = ()
                    else:
                        _, parent_span = source_unit(old, parent)
                        guard(text, item, namespace)
                        replacements.append((parent_span[1], parent_span[1], '\n' + text))
                        continue
                elif parts[-1] in bound_names(old) or '*' in bound_names(old):
                    raise ProofRefusal('observer shadows an existing non-definition binding', name, parts[-1])
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
                raise ProofRefusal('literal support cannot shadow a base wildcard binding', name, symbol)
            validate_partial_literal(text, new, name, symbol)
            imports.append(text)
            bound.add(symbol)
            inferred_support.add(item)
            identities[item] = digest(text)
        missing = required - bound
        if missing:
            raise ProofRefusal('unresolved observer symbol; add a new observer_support definition or local import',
                               name, sorted(missing)[0])
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
    for subject in subjects:
        if unit(destination, subject) != unit(base, subject):
            raise ProofRefusal('assembled proof changed a base subject binding', subject.split('::')[0])
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
