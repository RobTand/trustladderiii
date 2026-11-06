import importlib.util
from pathlib import Path
import sys

import pytest

from tl3_harness.trees import git

spec = importlib.util.spec_from_file_location('inventory', Path(__file__).parents[1] / 'tools/inventory.py')
inventory = importlib.util.module_from_spec(spec)
spec.loader.exec_module(inventory)


def test_node_ids_keep_methods_and_async_tests_but_exclude_disabled_helpers(tmp_path):
    (tmp_path / 'test_sample.py').write_text('''
def test_helper(): pass
test_helper.__test__ = False
async def test_async(): pass
class TestGroup:
    def test_method(self): pass
    async def test_async_method(self): pass
    def helper(self): pass
class Helper:
    def test_hidden(self): pass
def helper():
    def test_nested(): pass
''')
    assert inventory.node_ids(tmp_path, ['test_sample.py']) == [
        'test_sample.py::test_async', 'test_sample.py::TestGroup::test_method',
        'test_sample.py::TestGroup::test_async_method']


@pytest.mark.parametrize('per_file', [False, True])
def test_inventory_batches_selected_node_ids(tmp_path, monkeypatch, per_file):
    repo = tmp_path / 'source'
    repo.mkdir()
    git(repo, 'init', '-q')
    for name in ['test_first.py', 'test_second.py']:
        (repo / name).write_text('def test_ok(): pass\n')
    git(repo, 'add', '.')
    git(repo, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
        'commit', '-qm', 'fixture')
    bundle = tmp_path / 'served.bundle'
    git(repo, 'bundle', 'create', str(bundle), 'HEAD')
    image = 'sha256:' + '1' * 64
    monkeypatch.setattr(sys, 'argv', ['inventory.py', *(['--per-file'] if per_file else []),
                                    str(bundle), image, 'test_second.py', 'test_first.py'])
    calls = []

    def run_checks(repo, base, head, checks, *, container):
        assert base == head
        assert container.image == image
        calls.append(tuple(check.tests for check in checks))
        return ()

    monkeypatch.setattr(inventory, 'run_checks', run_checks)
    assert inventory.main() == 0
    first, second = ('test_first.py::test_ok',), ('test_second.py::test_ok',)
    expected = [(first,), (second,)] if per_file else [(first, second)]
    assert calls == expected


def test_node_ids_include_unittest_cases_without_a_test_prefix(tmp_path):
    (tmp_path / 'test_policy.py').write_text('''import unittest
from unittest import TestCase as Case
class PerformantPolicy(unittest.TestCase):
    def test_admission(self): pass
class OtherPolicy(Case):
    def test_refusal(self): pass
class Helper:
    def test_hidden(self): pass
''')
    assert inventory.node_ids(tmp_path, ['test_policy.py']) == [
        'test_policy.py::PerformantPolicy::test_admission',
        'test_policy.py::OtherPolicy::test_refusal']


def test_node_ids_follow_indirect_unittest_bases(tmp_path):
    (tmp_path / 'test_policy.py').write_text('''import unittest as unit
class PolicyBase(unit.TestCase):
    pass
class Intermediate(PolicyBase):
    def test_admission(self): pass
class Policy(Intermediate):
    def test_refusal(self): pass
class Helper:
    def test_hidden(self): pass
class OtherHelper(Helper):
    def test_also_hidden(self): pass
''')
    assert inventory.node_ids(tmp_path, ['test_policy.py']) == [
        'test_policy.py::Intermediate::test_admission',
        'test_policy.py::Policy::test_admission',
        'test_policy.py::Policy::test_refusal']


def test_rule_core_loads_standalone_and_repairs_its_table():
    import ast
    from dataclasses import replace
    from types import ModuleType
    source = Path(__file__).parents[1] / 'src/tl3_harness'
    for filename in ('rules.py', 'terminal_forms.py'):
        tree = ast.parse((source / filename).read_text())
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                assert all(alias.name.split('.')[0] in sys.stdlib_module_names for alias in node.names)
            elif isinstance(node, ast.ImportFrom):
                sibling_core = filename == 'terminal_forms.py' and node.level == 1 and node.module == 'rules'
                assert sibling_core or (node.level == 0 and node.module.split('.')[0] in sys.stdlib_module_names)
    package = ModuleType('staged_harness')
    package.__path__ = [str(source)]
    sys.modules[package.__name__] = package
    loaded = []
    try:
        for name in ('rules', 'terminal_forms'):
            spec = importlib.util.spec_from_file_location(package.__name__ + '.' + name, source / (name + '.py'))
            module = importlib.util.module_from_spec(spec)
            sys.modules[spec.name] = module
            loaded.append(spec.name)
            spec.loader.exec_module(module)
        core, forms = (sys.modules[name] for name in loaded)
        assert core.terminal_for('planner') is forms.PLANNER
        assert core.terminal_for('maker') is forms.MAKER
        field = core.Name('field', '/observer')
        allowed = core.Allowed('enum', ['assertion', 'exception'])
        before = {'observer': 'guess', 'untouched': True}
        row = core.register(core.Rule('proof.observer',
            lambda facts: () if allowed.accepts(facts['reply']['observer']) else (
                {'bad_value': facts['reply']['observer']},),
            lambda facts: (field, core.Name('path', 'tests/test_x.py'),
                           core.Name('symbol', 'tests/test_x.py::test_x')),
            lambda facts: allowed, lambda facts: core.Edit(field, 'replace', allowed),
            example={'reply': before}, witness='assertion'), roles=('planner', 'maker'))
        violation, = core.evaluate((row,), {'reply': before}, role='planner')
        rendered = violation.render(role='planner')
        assert all(item in rendered for item in ('/observer', 'source path', 'symbol', 'guess',
                                                 'assertion', 'exception', 'replace'))
        after = core.apply_edit(before, row.repair(violation.facts), 'assertion')
        assert core.permits(violation, before, after, role='planner')
        assert not core.permits(violation, before, {**after, 'untouched': 1}, role='planner')
        assert not core.Allowed('enum', [1]).accepts(True)
        with pytest.raises(ValueError, match='renderable repair'):
            core.register(replace(row, repair=lambda facts: None), roles=('planner',))
        with pytest.raises(ValueError, match='own witness'):
            core.register(replace(row, check=lambda facts: ({},)), roles=('planner',))
        with pytest.raises(ValueError, match='own witness'):
            core.register(replace(row, check=lambda facts: () if facts['reply']['observer'] == 'assertion'
                                  else ({'bad_value': facts['reply']['observer']},)), roles=('planner',))
        gate = core.register(replace(row, gating=True, repair=lambda facts: core.CantMeet('identity differs'),
                                     witness={'kind': 'cant_meet'}), roles=('planner', 'maker'))
        found = core.evaluate((gate, row), {'reply': before}, role='planner')
        assert 'Further checks were not run' in core.render_all(found, role='planner')
        assert forms.PLANNER.phrase in core.render_all(found, role='planner')
        assert forms.MAKER.phrase in core.render_all(found, role='maker')
        assert core.permits(gate, before, {'kind': 'cant_meet'}, role='planner')
        assert core.permits(found, before, {'kind': 'cant_meet'}, role='maker')
        assert not core.permits(found, before, after, role='planner')
        for roles in (('postmortem',), ('planner', 'postmortem')):
            with pytest.raises(ValueError, match='terminal reply form'):
                core.register(gate, roles=roles)
        with pytest.raises(ValueError, match='not declared'):
            core.evaluate((row,), {'reply': before}, role='postmortem')
        with pytest.raises(ValueError, match='not declared'):
            core.render_all(found, role='postmortem')
    finally:
        for name in reversed(loaded):
            del sys.modules[name]
        del sys.modules[package.__name__]


@pytest.mark.parametrize('role', ['planner', 'maker'])
def test_role_dependent_proof_repairs(role):
    from dataclasses import asdict, replace
    from tl3_harness import rules as core, terminal_forms as forms

    assert hasattr(core, 'SourceEdit'), 'proof rows cannot express source repairs'
    before = {'checks': [{'proof': {'subjects': ['wrong.py']}}], 'untouched': True}
    field = core.Name('field', '/checks/0/proof/subjects/0')
    path = 'tests/test_observer.py'
    allowed = core.Allowed('const', 'subject.py')
    source = core.SourceEdit(path, 'Move this call into the observer test body.')
    row = core.register(core.Rule(
        'proof.role_repair',
        lambda facts: () if core.at(facts['reply'], field.value) == 'subject.py' else ({'path': path},),
        lambda facts: (core.Name('path', facts['path']),),
        lambda facts, role: allowed if role == 'planner' else core.Allowed('const', path),
        lambda facts, role: core.Edit(field, 'replace', allowed) if role == 'planner' else source,
        example={'reply': before}, witness={
            'planner': core.apply_edit(before, core.Edit(field, 'replace', allowed), 'subject.py'),
            'maker': None}), roles=('planner', 'maker'))
    violation, = core.evaluate((row,), {'reply': before}, role=role)
    text = violation.render(role=role)
    with pytest.raises(ValueError, match='evaluation role'):
        violation.render(role='maker' if role == 'planner' else 'planner')
    assert forms.PLANNER.phrase not in text and forms.MAKER.phrase not in text
    if role == 'planner':
        assert field.value in text and 'replace' in text
        after = core.apply_edit(before, violation.repair, 'subject.py')
        assert core.permits(violation, before, after, role=role)
        assert not core.permits(violation, before, {**after, 'untouched': False}, role=role)
    else:
        assert f'Change "{path}": {source.instruction}' in text
        assert asdict(violation)['repair'] == {'path': path, 'instruction': source.instruction}
        assert asdict(violation)['role'] == role
        assert core.permits(violation, before, before, role=role)
        assert not core.permits(violation, before, {'kind': 'cant_meet'}, role=role)
        with pytest.raises(ValueError, match='source editor'):
            core.register(replace(row, repair=source), roles=('planner',))
        rogue = replace(row, repair=lambda facts, role: source)
        with pytest.raises(ValueError, match='source editor'):
            core.evaluate((rogue,), {'reply': before}, role='planner')
        with pytest.raises(ValueError, match='source editor'):
            replace(violation, rule=rogue, role='planner').render(role='planner')
    with pytest.raises(ValueError, match='terminal reply form'):
        core.register(replace(row, repair=core.CantMeet('declaration is wrong')),
                      roles=('postmortem',))


def test_resolved_rule_values_are_stable():
    from tl3_harness import rules as core
    allowed = core.Allowed('const', 'ok')
    row = core.register(core.Rule('proof.static',
        lambda facts: () if facts['reply']['value'] == 'ok' else ({},),
        lambda facts: (core.Name('field', '/value'),), allowed,
        core.Edit(core.Name('field', '/value'), 'replace', allowed),
        example={'reply': {'value': 'bad'}}, witness='ok'), roles=('planner',))
    violation, = core.evaluate((row,), row.example, role='planner')
    assert violation.allowed == allowed
    assert violation.repair == row.repair
    assert core.permits(violation, row.example['reply'], {'value': 'ok'}, role='planner')


@pytest.mark.parametrize('same', [True, False])
@pytest.mark.parametrize('op', ['add', 'replace', 'remove'])
def test_edit_allowed_sets_render_once_only_when_equal(same, op):
    from tl3_harness import rules as core
    allowed = core.Allowed('enum', ['ValueError', 'TypeError'])
    edit_allowed = core.Allowed('enum', list(allowed.value) if same else ['RuntimeError'])
    row = core.Rule('forcing.raise_class', lambda facts: ({},),
        lambda facts: (core.Name('field', '/checks/0/forcing/patches/0/raises'),),
        allowed, core.Edit(core.Name('field', '/checks/0/forcing/patches/0/raises'),
                           op, edit_allowed), roles=frozenset({'planner'}))
    violation, = core.evaluate((row,), {}, role='planner')
    text = violation.render(role='planner')
    assert text.count(allowed.render()) == 1
    if same:
        assert ('with one of the allowed values above' in text) == (op != 'remove')
    else:
        assert edit_allowed.render() in text


def test_per_role_witnesses_cover_edit_terminal_and_source_repairs():
    from dataclasses import replace
    from tl3_harness import rules as core
    before = {'value': 'bad'}
    allowed = core.Allowed('const', 'ok')
    row = core.Rule('proof.role_witness',
        lambda facts: () if facts['reply']['value'] == 'ok' else ({},),
        lambda facts: (core.Name('path', 'tests/test_observer.py'),), allowed,
        lambda facts, role: core.Edit(core.Name('field', '/value'), 'replace', allowed)
            if role == 'planner' else core.CantMeet('declaration must change'),
        example={'reply': before},
        witness={'planner': {'value': 'ok'}, 'maker': {'kind': 'cant_meet'}})
    core.register(row, roles=('planner', 'maker'))
    for witness in ({}, {'planner': {'value': 'ok'}}, {'maker': {'kind': 'cant_meet'}}):
        with pytest.raises(ValueError, match='witness.*role'):
            core.register(replace(row, witness=witness), roles=('planner', 'maker'))
    with pytest.raises(ValueError, match='own witness'):
        core.register(replace(row, witness={'planner': {'value': 'ok', 'extra': 1},
                                           'maker': {'kind': 'cant_meet'}}), roles=('planner', 'maker'))
    source = core.SourceEdit('tests/test_observer.py', 'Move the call into the test body.')
    core.register(replace(row, repair=source, witness={'maker': None}), roles=('maker',))
    with pytest.raises(ValueError, match='no reply witness'):
        core.register(replace(row, repair=source, witness={'maker': {'value': 'ok'}}), roles=('maker',))


@pytest.mark.parametrize('path,instruction', [('', 'fix'), ('../test_x.py', 'fix'),
    ('tests/../test_x.py', 'fix'), ('/test_x.py', 'fix'), ('tests/test_x.py', ''),
    ('tests/test_x.py', '   ')])
def test_source_edit_has_a_relative_path_and_instruction(path, instruction):
    from tl3_harness.rules import SourceEdit
    with pytest.raises(ValueError):
        SourceEdit(path, instruction)


@pytest.mark.parametrize('repair_kind', ['cant_meet', 'source', 'edit'])
def test_render_context_preserves_reply_and_distinguishes_tool_repairs(repair_kind):
    from tl3_harness import rules as core, terminal_forms as forms

    reason = 'Declare it via needs_files, or use run_tests/grep/read.'
    allowed = core.Allowed('const', 'tests/test_observer.py')
    repairs = {
        'cant_meet': core.CantMeet(reason),
        'source': core.SourceEdit('tests/test_observer.py', 'Move the call into the test body.'),
        'edit': core.Edit(core.Name('field', '/value'), 'replace', allowed),
    }
    row = core.Rule('maker.footprint', lambda facts: ({},),
        lambda facts: (core.Name('path', 'tests/test_observer.py'),),
        allowed, repairs[repair_kind], roles=frozenset({'maker'}))
    violation, = core.evaluate((row,), {}, role='maker')
    reply = violation.render(role='maker')
    assert violation.render(role='maker', context='reply') == reply
    assert core.render_all((violation,), role='maker', context='reply') == reply
    for render in (lambda: violation.render(role='maker', context='tool'),
                   lambda: core.render_all((violation,), role='maker', context='tool')):
        if repair_kind == 'edit':
            with pytest.raises(ValueError, match='Edit.*tool'):
                render()
        elif repair_kind == 'source':
            assert render() == reply
        else:
            assert forms.MAKER.phrase in reply
            assert render() == reply.replace(forms.MAKER.phrase + ' ', '')
            assert reason in render()
            assert forms.MAKER.phrase not in render()
