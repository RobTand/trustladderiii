"""Proof-assembly failure facts and their shared, role-aware refusal rows.

The Python assembler owns these facts until the proof-assembly spike decides
where the checks live. Source changes cannot be granted as declaration edits.
"""
import builtins
from dataclasses import replace

from .terminal_forms import PLANNER, MAKER
from .rules import Allowed, CantMeet, Edit, Name, Rule, Violation, at, register, render_all


PURE_SUPPORT_BUILTINS = frozenset({
    'abs', 'all', 'any', 'bool', 'bytes', 'enumerate', 'float', 'frozenset',
    'int', 'len', 'max', 'min', 'range', 'repr', 'reversed', 'round',
    'sorted', 'str', 'sum', 'tuple', 'zip'})
NAMESPACE_API_NAMES = frozenset({
    'globals', 'locals', 'vars', 'eval', 'exec', 'compile', '__import__',
    'getattr', 'setattr', 'delattr', '__builtins__', 'attrgetter', 'methodcaller'})
NAMESPACE_API_ATTRIBUTES = frozenset({
    '__dict__', '__globals__', '__builtins__', '__getattribute__',
    '__setattr__', '__delattr__', 'f_globals', 'f_locals', 'gi_frame',
    'cr_frame', 'tb_frame', '_getframe', 'currentframe', 'modules', 'attrgetter', 'methodcaller'})
NAMESPACE_API_MODULES = frozenset({'builtins', 'sys', 'inspect', 'importlib', '__main__'})
REFLECTIVE_SUPPORT_ATTRIBUTES = frozenset({'format', 'format_map', 'Formatter', 'TestCase', 'main'})
LOADER_STDLIB = frozenset({'pathlib', 'unittest', 'importlib'})
SOURCE_DEFINITION_COUNT = 1
STAGED_CHECK_COUNT = 1


def forced_exception(name):
    """The predicate shared by forcing validation and its advertised choices."""
    error = getattr(builtins, name, None) if name.isidentifier() else None
    if isinstance(error, type) and issubclass(error, Exception) and not issubclass(error, AssertionError):
        return error
    return None


def symbol_name(module, selector):
    """Convert declaration/pytest levels only at the shared Name boundary."""
    levels = selector.split('::')
    if levels[0] == module:
        levels = levels[1:]
    qualname = '.'.join(levels)
    if qualname and all(part.isidentifier() for part in qualname.split('.')):
        return Name('symbol', module + '::' + qualname)
    return None


def names(facts):
    result = [Name('field', facts.get('field', '/proof'))]
    if module := facts.get('module'):
        result.append(Name('path', module))
        if symbol := facts.get('symbol'):
            if name := symbol_name(module, symbol):
                result.append(name)
    if path := facts.get('path'):
        if path != facts.get('module'):
            result.append(Name('path', path))
    return tuple(result)


def render(violations, role):
    """The only adapter from proof facts to the shared role-aware renderer."""
    return render_all(violations, role=role)


def failure_facts(facts):
    """The assembler calls a refusal row only after establishing its failure.

    These rows consume already-established source facts. Their repair is a
    terminal reply, so there is no declaration value that can satisfy them.
    """
    return (facts,)


def source_row(id, allowed, reason, *, gating=False):
    return Rule(id, failure_facts, names, lambda facts: Allowed('const', allowed),
                lambda facts: CantMeet(reason.format_map(facts)), gating=gating)


ROWS = {}


def add(area, rule, allowed, reason, *, gating=False):
    row = source_row('proof.' + area + '.' + rule, allowed, reason, gating=gating)
    ROWS[row.id] = row


add('source', 'definition', {'definitions': SOURCE_DEFINITION_COUNT},
    'Select one existing, unambiguous source definition in the declaration.', gating=True)
add('source', 'unparsable', {'source': 'Python accepted by both ast and symtable'},
    'The selected source cannot be parsed or scoped. Correct the Python source before declaring this proof.', gating=True)
add('tree', 'destination_fresh', {'destination': 'a fresh tree'},
    'Proof assembly needs a fresh staging tree. The caller must supply a new destination.', gating=True)
add('subject', 'overlap', {'subject': 'disjoint from observers and observer_support'},
    'Select changed subject source separately from observer tests and their support.', gating=True)
add('subject', 'changed', {'subject': 'source that differs between base and head'},
    'Select the changed source in /proof/subjects; an unchanged unit cannot establish this proof.', gating=True)
add('subject', 'preserved', {'subject': 'byte-identical base source in the assembled proof'},
    'The selected partition changes a base subject binding. Separate the subject from observer context.', gating=True)
add('support', 'new', {'support': 'a new binding absent from base'},
    'Select new observer support. Keep existing base subjects outside observer_support.', gating=True)
add('observer', 'overlap', {'observers': 'disjoint exact selections'},
    'Select each observer once, using exact methods when selecting class members.', gating=True)
add('observer', 'test_path', {'observer': 'inside configured test-files'},
    'Select an observer inside the repository configured tests.', gating=True)
add('observer', 'module_missing', {'observer': 'a module present in head'},
    'Select an observer module that exists in the head tree.', gating=True)
add('observer', 'subject_module', {'observer': 'a new module containing no declared subject'},
    'Separate the new observer module from the declared subject source.', gating=True)
add('observer', 'subject_class', {'observer': 'a new class containing no declared subject'},
    'Select exact observer methods or separate the new class from the declared subject.', gating=True)
add('observer', 'exact_method', {'observer': 'an exact test method, rather than an existing whole class'},
    'Select the exact observer methods in /tests instead of the whole class.', gating=True)
add('observer', 'binding_shadow', {'observer': 'a definition without a conflicting base binding'},
    'Use a new observer name or an existing test definition, preserving the base binding.', gating=True)
add('observer', 'unresolved_symbol', {'observer': 'all globals bound by base or new observer support'},
    'Add a new observer_support definition for the missing name, or use a local import in the observer test body.')
add('support', 'graft_binding', {'support': 'new bindings disjoint from base and subjects'},
    'Use new plain support names; move setup into the observer test body instead of rebinding base source.', gating=True)
add('support', 'existing_module', {'support': 'a new module or a new exact definition'},
    'Select exact new definitions from an existing module; an existing whole module cannot be observer support.', gating=True)
add('support', 'literal_wildcard', {'support': 'no base wildcard binding with the same name'},
    'Use a local observer value or import; inferred literal support must preserve base wildcard bindings.')
add('support', 'definition_write', {'partial_definition': 'no member or subscript writes elsewhere'},
    'Move the setup into the observer test body or a new support factory, preserving the partial definition.')
add('support', 'definition_use', {'partial_definition': 'only direct calls elsewhere in the module'},
    'Move other uses into the observer test body so the selected support definition is self-contained.')
add('support', 'definition_binding', {'partial_definition': 'bound exactly once'},
    'Remove later rebinding, or move setup into the observer test body or a new support factory.')
add('support', 'literal_initializer', {'initializer': 'immutable literal syntax without calls'},
    'Put initialization inside the observer test body or a new support factory.')
add('support', 'literal_assignment', {'literal_support': 'one plain name in one assignment'},
    'Put test data inside the observer test body or a new support factory.')
add('support', 'literal_binding', {'literal_support': 'immutable and bound exactly once'},
    'Put test data inside the observer test body or a new support factory, including its initialization context.')
add('support', 'wildcard_import', {'imports': 'explicit names'},
    'Import each needed name explicitly, or move this into the observer test body.')
add('support', 'namespace_api', {
    'excluded_names': sorted(NAMESPACE_API_NAMES), 'excluded_attributes': sorted(NAMESPACE_API_ATTRIBUTES),
    'excluded_modules': sorted(NAMESPACE_API_MODULES)},
    'Use plain support bindings; move dynamic namespace operations into the observer test body.')
add('support', 'decorator', {'support_definition': 'undecorated'},
    'Move the fixture body into the observer test body, or declare the fixture module as a new observer module. '
    'A plain undecorated support function called by the observer is also allowed; @pytest.fixture remains refused.')
add('support', 'class_bases', {'support_class': 'no base classes or class keywords'},
    'Use a plain class without bases, or move this into the observer test body.')
add('support', 'context_manager', {'with': 'scoped sys.argv setup with mock.patch.object from unittest.mock'},
    'Move the with block into the observer test body; support admits only the scoped sys.argv setup.')
add('support', 'call_allowlist', {
    'builtins': sorted(PURE_SUPPORT_BUILTINS), 'builtin_arguments': 'literal positional arguments only',
    'functions': 'top-level functions defined once in this support'},
    'Move any check that inspects a parameter or variable into the observer test body. '
    'For string formatting use an f-string over local values there.')
add('support', 'percent_format', {'template': 'literal str or bytes without __', 'operands': 'literal'},
    'Use an f-string over local values in the observer test body.')
add('support', 'reflective_attribute', {'excluded_attributes': sorted(REFLECTIVE_SUPPORT_ATTRIBUTES)},
    'Use an f-string, or move this into the observer test body.')
add('support', 'member_write', {'support_binding': 'brand-new plain names without member or subscript writes'},
    'Build the value and bind it to a new name, or move this into the observer test body.')
add('loader', 'stdlib_shadow', {'trusted_modules': sorted(LOADER_STDLIB),
    'import_roots': 'repository root, pytest pythonpath entries, or directories without __init__.py',
    'loader': 'no repository file importable under a trusted standard-library name'},
    '{path} could shadow the standard-library module `{stdlib}` that the observer loader trusts, because it is '
    'a top-level name in an import root (the repository root, a pytest pythonpath entry, or a directory without '
    '__init__.py); rename or move it if this change owns it, otherwise (for example a vendored dependency) put '
    'the observer and its loading code in the test body of a test module that already exists on the base commit.')
add('staging', 'single_check', {'checks': STAGED_CHECK_COUNT},
    'Stage one check at a time; each check has its own source partition.', gating=True)


def forcing_allowed(facts):
    return Allowed('enum', sorted(name for name in dir(builtins) if forced_exception(name)))


def forcing_failures(facts):
    value = at(facts['reply'], facts['field'])
    return () if forcing_allowed(facts).accepts(value) else ({**facts, 'bad_value': value},)


FORCING_RAISE_CLASS = Rule(
    'forcing.raise_class', forcing_failures, lambda facts: (Name('field', facts['field']),), forcing_allowed,
    lambda facts: Edit(Name('field', facts['field']), 'replace', forcing_allowed(facts)))
ROWS[FORCING_RAISE_CLASS.id] = FORCING_RAISE_CLASS


def example(row):
    facts = {'reply': {'proof': {'subjects': ['app.py::value'], 'observer_support': ['tests/test_observer.py']},
                       'tests': ['tests/test_observer.py::test_observer']},
             'module': 'tests/test_observer.py', 'symbol': 'helper', 'field': '/proof/observer_support/0',
             'path': 'pathlib.py', 'stdlib': 'pathlib'}
    if row is FORCING_RAISE_CLASS:
        facts.update(reply={'forcing': {'patches': [{'raises': 'AssertionError'}]}}, field='/forcing/patches/0/raises')
    return facts


for _row in ROWS.values():
    _example = example(_row)
    _witness = 'RuntimeError' if _row is FORCING_RAISE_CLASS else {'kind': 'cant_meet'}
    ROWS[_row.id] = register(replace(_row, example=_example, witness=_witness),
                             roles=(PLANNER.role, MAKER.role))
FORCING_RAISE_CLASS = ROWS[FORCING_RAISE_CLASS.id]


def violation_data(violation):
    """Serialize the shared row without copying executable callbacks to the wire."""
    row, facts = violation.rule, violation.facts
    allowed, repair = row.allowed(facts), row.repair(facts)
    names_data = [{'kind': item.kind, 'value': item.value} for item in row.names(facts)]
    repair_data = ({'kind': 'Edit', 'pointer': {'kind': repair.pointer.kind, 'value': repair.pointer.value},
                    'op': repair.op, 'allowed': {'keyword': repair.allowed.keyword, 'value': repair.allowed.value}}
                   if isinstance(repair, Edit) else {'kind': 'CantMeet', 'reason': repair.reason})
    return {'id': row.id, 'names': names_data, 'allowed': {'keyword': allowed.keyword, 'value': allowed.value},
            'repair': repair_data, 'gating': row.gating,
            **({'bad_value': facts['bad_value']} if 'bad_value' in facts else {})}


def export_rows():
    """Planner preflight templates, generated from the executable Python rows."""
    result = []
    for id, row in sorted(ROWS.items()):
        if id.startswith(('proof.loader.', 'proof.support.')):
            facts = example(row)
            data = violation_data(Violation(row, facts))
            if id == 'proof.loader.stdlib_shadow':
                data['repair']['reason'] = data['repair']['reason'].replace('pathlib.py', '{path}').replace('`pathlib`', '`{stdlib}`')
            data['names'] = [{'kind': name['kind'], 'value': '{' + name['kind'] + '}'} for name in data['names']]
            result.append(data)
    return result

