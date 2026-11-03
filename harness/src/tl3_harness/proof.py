"""The source partition shared by rehearsal, development tests, and Checking."""
import ast
import builtins
import hashlib
import json
from pathlib import Path
import shutil
import symtable
import textwrap

from .model import Kind, ProofRefusal
from .trees import bound_names, is_test, tree_sha256


RULE = 'subject-observer-v1'
INTEGRITY = '.tl3-proof-integrity.json'


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
    return source_unit(path.read_text(), identity, optional=True)[0]


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


def deny_namespace_apis(source, module):
    """Support is declarative source, never access to an execution namespace."""
    names = {'globals', 'locals', 'vars', 'eval', 'exec', 'compile', '__import__',
             'getattr', 'setattr', 'delattr', '__builtins__', '__tl3_proof_integrity__'}
    attributes = {'__dict__', '__globals__', '__builtins__', '__getattribute__',
                  '__setattr__', '__delattr__', 'f_globals', 'f_locals', 'gi_frame',
                  'cr_frame', 'tb_frame', '_getframe', 'currentframe', 'modules'}
    modules = {'builtins', 'sys', 'inspect', 'importlib', '__main__'}
    for node in ast.walk(ast.parse(textwrap.dedent(source))):
        symbol = None
        if isinstance(node, ast.Name) and node.id in names:
            symbol = node.id
        elif isinstance(node, ast.Attribute) and node.attr in attributes:
            symbol = node.attr
        elif isinstance(node, (ast.Import, ast.ImportFrom)):
            imported = [node.module or ''] if isinstance(node, ast.ImportFrom) else [alias.name for alias in node.names]
            symbol = next((name for name in imported if name.split('.')[0] in modules), None)
        if symbol:
            raise ProofRefusal('dynamic namespace API in observer/support can rebind base subjects', module, symbol)


def guard_runtime(source, name, protected, grafts=()):
    """Describe runtime checks without changing the subject/observer source bytes."""
    if '__tl3_proof_integrity__' in source:
        raise ProofRefusal('reserved proof integrity binding', name, '__tl3_proof_integrity__')
    return {'bindings': sorted(protected), 'grafts': [
        {'span': source_unit(source, item)[1], 'namespace': item.split('::')[1:-1]}
        for item in sorted(grafts)]}


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
    if any((tree / INTEGRITY).exists() for tree in (base, head)):
        raise ProofRefusal('reserved proof integrity manifest', INTEGRITY)
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
    identities = {}
    runtime = {}
    for name in sorted(paths):
        if not is_test(name, config['test-files']) and any(item.split('::')[0] == name for item in observers):
            raise ProofRefusal('observer is outside configured tests', name)
        target, old_path, new_path = destination / name, base / name, head / name
        if not new_path.is_file():
            raise ProofRefusal('missing head observer module', name)
        old = old_path.read_text() if old_path.is_file() else ''
        new = new_path.read_text()
        requested = [item for item in (*observers, *support) if item.split('::')[0] == name]
        for statement in ast.parse(new).body:
            if (isinstance(statement, (ast.Import, ast.ImportFrom))
                    and bound_names(ast.unparse(statement)) & {item.split('::')[-1] for item in requested}):
                deny_namespace_apis(ast.unparse(statement), name)
        if not old_path.exists():
            if any(subject.split('::')[0] == name for subject in subjects):
                raise ProofRefusal('new observer module would carry a declared subject onto base', name)
            # A new module has no old subject to overwrite. Its complete source is
            # the observer context, including its imports and pytest fixtures.
            for item in requested:
                identities[item] = digest(source_unit(new, item)[0])
            deny_namespace_apis(new, name)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(new)
            continue
        replacements, grafts, required_texts = [], [], []
        protected = written_bindings(old)
        protected.update(tuple(subject.split('::')[1:]) for subject in subjects if subject.split('::')[0] == name)
        protected.difference_update(tuple(item.split('::')[1:]) for item in observers if item.split('::')[0] == name)
        runtime_units = set(requested)
        def guard(text, item, namespace=()):
            deny_namespace_apis(text, name)
            permitted = {tuple(item.split('::')[1:])} if item in observers else set()
            written = written_bindings(text, namespace)
            conflict = (written & protected) - permitted
            if conflict or ('*',) in written:
                symbol = '::'.join(sorted(conflict)[0]) if conflict else 'globals'
                raise ProofRefusal('observer/support graft would rebind a base or subject binding', name, symbol)
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
                        runtime_units.discard(item)
                        runtime_units.add(parent)
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
        missing = required - bound
        if missing:
            raise ProofRefusal('unresolved observer symbol; add a new observer_support definition or local import',
                               name, sorted(missing)[0])
        # Imports follow a possible module docstring and __future__ statements.
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
        target.write_text(''.join(lines))
        runtime[name] = (protected, runtime_units)
    for subject in subjects:
        if unit(destination, subject) != unit(base, subject):
            raise ProofRefusal('assembled proof changed a base subject binding', subject.split('::')[0])
    if not outcome_observer or support:
        for name in {subject.split('::')[0] for subject in subjects} - runtime.keys():
            if (base / name).is_file():
                runtime[name] = (written_bindings((base / name).read_text()), ())
        if runtime:
            (destination / INTEGRITY).write_text(json.dumps({
                name: guard_runtime((destination / name).read_text(), name, protected, grafts)
                for name, (protected, grafts) in sorted(runtime.items())}, sort_keys=True))
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
            'support_sha256': {name: identities[name] for name in support},
            'source_base_tree_sha256': tree_sha256(base),
            'base_tree_sha256': tree_sha256(destination), 'head_tree_sha256': tree_sha256(head)}
