"""Proof-assembly failure facts and their shared, role-aware refusal rows.

The Python assembler owns these facts until the proof-assembly spike decides
where the checks live. Source changes cannot be granted as declaration edits.
"""
import builtins
from dataclasses import replace
from pathlib import PurePosixPath

from .rules import Rule, at, evaluate_facts as core_evaluate, register_rows as register, resolved_row
from .action_policy import row_membership, render_rows as render_all


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
SUPPORT_MOCK_IMPORTS = (('mock', 'from unittest import mock', 'mock'),
                        ('unittest', 'import unittest.mock', 'unittest.mock'))
SUPPORT_UNITTEST_IMPORTS = frozenset({'import unittest', *(source for _, source, _ in SUPPORT_MOCK_IMPORTS)})
SUPPORT_ARGV_ATTRIBUTE = 'argv'
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
        return {'kind': 'symbol', 'value': module + '::' + qualname}
    return None


def names(facts):
    source = repair_for(facts, facts.get('role', 'maker'), {'kind': 'schema', 'keyword': 'const', 'value': None}, 'Source correction')
    result = ([] if source['kind'] == 'source_edit' else
              [{'kind': 'field', 'value': facts.get('field', declaration_pointer('/proof', facts.get('check_index', 0)))}])
    if module := facts.get('module'):
        result.append({'kind': 'path', 'value': module})
        if symbol := facts.get('symbol'):
            if name := symbol_name(module, symbol):
                result.append(name)
    if path := facts.get('path'):
        if path != facts.get('module'):
            result.append({'kind': 'path', 'value': path})
    return tuple(result)


def evaluate(rows, facts, *, role):
    """Bind the caller role for names, whose core callback receives facts only."""
    return core_evaluate(rows, {**facts, 'role': role}, role=role)


def render(violations, role):
    """The only adapter from proof facts to the shared role-aware renderer."""
    return render_all([resolved_row(row, facts, role) for row, facts in violations], role=role)


def declaration_pointer(field, check_index, role='maker'):
    """The sole boundary from check-relative paths to planner reply pointers."""
    pointer = field if field.startswith('/checks') else f'/checks/{check_index}' + field
    test_prefix = f'/checks/{check_index}/tests'
    if role == 'planner' and (pointer == test_prefix or pointer.startswith(test_prefix + '/')):
        return f'/checks/{check_index}/tests' + pointer[len(test_prefix):]
    return pointer


def failure_facts(facts):
    # Alternatives have been checked by the assembler on the actual source trees.
    if facts.get('alternatives') and 'reply' in facts:
        try:
            if row_membership(selection_allowed(facts), at(facts['reply'], facts['field'])):
                return ()
        except (KeyError, IndexError, TypeError):
            pass
    return (facts,)


def selection_allowed(facts):
    return {'kind': 'schema', 'keyword': 'enum', 'value': facts['alternatives']}


def repair_for(facts, role, allowed, reason):
    """One role adapter: declaration edits, footprint edits, or a specific terminal reason."""
    rule = facts.get('rule', '')
    if role == 'planner' and facts.get('alternatives'):
        return {'kind': 'edit', 'pointer': {'kind': 'field', 'value': facts['field']}['value'], 'op': 'replace', 'allowed': allowed}
    if rule == 'proof.tree.destination_fresh':
        return {'kind': 'cant_meet', 'reason': 'TL3-side assembly fault: the harness supplied a staging destination that already exists.'}
    if rule == 'proof.staging.single_check':
        return {'kind': 'cant_meet', 'reason': 'stage one check at a time'}
    path = facts.get('path') if rule == 'proof.loader.stdlib_shadow' else facts.get('module')
    source_rule = (rule.startswith(('proof.support.', 'proof.loader.'))
                   and rule not in ('proof.support.new', 'proof.support.existing_module')
                   or rule in ('proof.source.unparsable', 'proof.source.definition',
                               'proof.observer.module_missing', 'proof.observer.unresolved_symbol'))
    blocked = path and any(PurePosixPath(path).full_match(pattern)
                           for pattern in (*facts.get('protected', ()), *facts.get('generated', ())))
    if role == 'maker' and source_rule and path in (facts.get('footprint') or ()) and not blocked:
        if rule == 'proof.source.unparsable':
            reason = f"fix the syntax error at line {facts.get('line', '?')}: {facts.get('bad_value', 'invalid Python')}"
        instructions = {
            'proof.source.definition': 'Define the declared definition in this module, preserving the declared source partition.',
            'proof.observer.module_missing': 'Create this declared observer module and its selected test definitions.',
            'proof.observer.unresolved_symbol': 'Define the missing name in the observer module or inside the observer test body.',
            'proof.support.decorator': 'Move the fixture body into the observer test body and remove the decorated support dependency.',
            'proof.loader.stdlib_shadow': 'Move or rename this shadowing file so it is no longer importable under the trusted standard-library name.',
        }
        return {'kind': 'source_edit', 'path': path, 'instruction': instructions.get(rule, reason)}
    if role == 'maker' and rule == 'proof.loader.stdlib_shadow':
        reason += ' The observer must move to an existing base test module, which requires a plan change.'
    if role == 'planner' and facts.get('selection_search'):
        reason += ' No concrete declaration selection passes assembly on these trees.'
    return {'kind': 'cant_meet', 'reason': reason}


def source_row(id, allowed, reason, *, gating=False):
    def choices(facts, role):
        if role == 'planner' and facts.get('alternatives'):
            return selection_allowed(facts)
        value = allowed(facts) if callable(allowed) else allowed
        return {'kind': 'schema', 'keyword': 'const', 'value': value}
    def repair(facts, role):
        explanation = reason.format_map(facts)
        if facts.get('why'):
            explanation = facts['why'] + '. ' + explanation
        return repair_for({**facts, 'rule': id}, role, choices(facts, role), explanation)
    def failures(facts):
        return tuple({**item, 'rule': id} for item in failure_facts(facts))
    return Rule(id, failures, names, choices, repair, gating=gating)


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
add('observer', 'test_path', lambda facts: {'test-files': facts.get('test_files', [])},
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
add('support', 'context_manager', {
    'context_managers': [root + f'.patch.object(sys, "{SUPPORT_ARGV_ATTRIBUTE}", value)' for _, _, root in SUPPORT_MOCK_IMPORTS],
    'imports': sorted(SUPPORT_UNITTEST_IMPORTS | {'import sys'}), 'keywords': [], 'scope': 'with'},
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


def forcing_allowed(facts, role=None):
    return {'kind': 'schema', 'keyword': 'enum', 'value': sorted((name for name in dir(builtins) if forced_exception(name)))}


def forcing_failures(facts):
    value = at(facts['reply'], facts['field'])
    return () if row_membership(forcing_allowed(facts), value) else ({**facts, 'bad_value': value},)


FORCING_RAISE_CLASS = Rule(
    'forcing.raise_class', forcing_failures, lambda facts: ({'kind': 'field', 'value': facts['field']},), forcing_allowed,
    lambda facts, role: ({'kind': 'edit', 'pointer': {'kind': 'field', 'value': facts['field']}['value'], 'op': 'replace', 'allowed': forcing_allowed(facts)}
                         if role == 'planner' else {'kind': 'cant_meet', 'reason': 'The declared forcing exception class requires a planner declaration repair.'}))
ROWS[FORCING_RAISE_CLASS.id] = FORCING_RAISE_CLASS


def example(row):
    facts = {'reply': {'checks': [{'proof': {'subjects': ['app.py::value'], 'observer_support': ['tests/test_observer.py']},
                       'tests': ['tests/test_observer.py::test_observer']}]},
             'module': 'tests/test_observer.py', 'symbol': 'helper', 'field': '/checks/0/proof/observer_support/0',
             'path': 'pathlib.py', 'stdlib': 'pathlib'}
    if row is FORCING_RAISE_CLASS:
        facts.update(reply={'checks': [{'forcing': {'patches': [{'raises': 'AssertionError'}]}}]}, field='/checks/0/forcing/patches/0/raises')
    return facts


for _row in ROWS.values():
    _example = example(_row)
    _witness = ({'planner': {'checks': [{'forcing': {'patches': [{'raises': 'RuntimeError'}]}}]},
                 'maker': {'kind': 'cant_meet'}} if _row is FORCING_RAISE_CLASS else
                {'planner': {'kind': 'cant_meet'}, 'maker': {'kind': 'cant_meet'}})
    ROWS[_row.id] = register(replace(_row, example=_example, witness=_witness),
                             roles=('planner', 'maker'))
FORCING_RAISE_CLASS = ROWS[FORCING_RAISE_CLASS.id]


def violation_data(violation, role):
    """Resolve callback facts into the one canonical Rust-owned row wire."""
    row, facts = violation
    return resolved_row(row, facts, role)
