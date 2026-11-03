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
                        or check.declared_symptom is not None or check.forcing is not None and declaration is None)
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
    for name in sorted(paths):
        if not is_test(name, config['test-files']) and any(item.split('::')[0] == name for item in observers):
            raise ProofRefusal('observer is outside configured tests', name)
        target, old_path, new_path = destination / name, base / name, head / name
        if not new_path.is_file():
            raise ProofRefusal('missing head observer module', name)
        old = old_path.read_text() if old_path.is_file() else ''
        new = new_path.read_text()
        requested = [item for item in (*observers, *support) if item.split('::')[0] == name]
        if not old_path.exists():
            # A new module has no old subject to overwrite. Its complete source is
            # the observer context, including its imports and pytest fixtures.
            for item in requested:
                identities[item] = digest(source_unit(new, item)[0])
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(new)
            continue
        replacements, grafts, required_texts = [], [], []
        for item in requested:
            text, _ = source_unit(new, item)
            required_texts.append(text)
            if '::' not in item:
                raise ProofRefusal('an existing module cannot be observer support', name)
            identities[item] = digest(text)
            try:
                _, span = source_unit(old, item)
            except ProofRefusal:
                span = None
                parts = item.split('::')
                if len(parts) > 2:
                    # A new class carries the method's class and fixture context.
                    parent = '::'.join(parts[:2])
                    try:
                        source_unit(old, parent)
                    except ProofRefusal:
                        text, _ = source_unit(new, parent)
                    else:
                        _, parent_span = source_unit(old, parent)
                        replacements.append((parent_span[1], parent_span[1], '\n' + text))
                        continue
                elif parts[-1] in bound_names(old) or '*' in bound_names(old):
                    raise ProofRefusal('observer shadows an existing non-definition binding', name, parts[-1])
            if span:
                replacements.append((*span, text))
            elif text not in grafts:
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
