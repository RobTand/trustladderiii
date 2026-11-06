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
    path = Path(__file__).parents[1] / 'src/tl3_harness/rules.py'
    tree = ast.parse(path.read_text())
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            assert all(alias.name.split('.')[0] in sys.stdlib_module_names for alias in node.names)
        elif isinstance(node, ast.ImportFrom):
            assert node.level == 0 and node.module.split('.')[0] in sys.stdlib_module_names
    spec = importlib.util.spec_from_file_location('staged_rules', path)
    core = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = core
    try:
        spec.loader.exec_module(core)
        field = core.Name('field', '/observer')
        allowed = core.Allowed('enum', ['assertion', 'exception'])
        row = core.Rule('proof.observer',
            lambda facts: () if allowed.accepts(facts['reply']['observer']) else (
                {'bad_value': facts['reply']['observer']},),
            lambda facts: (field, core.Name('path', 'tests/test_x.py'),
                           core.Name('symbol', 'tests/test_x.py::test_x')),
            lambda facts: allowed, lambda facts: core.Edit(field, 'replace', allowed))
        before = {'observer': 'guess', 'untouched': True}
        core.register(row, {'reply': before}, 'assertion')
        violation, = core.evaluate((row,), {'reply': before})
        rendered = violation.render()
        assert all(item in rendered for item in ('/observer', 'source path', 'symbol', 'guess',
                                                 'assertion', 'exception', 'replace'))
        after = core.apply_edit(before, row.repair(violation.facts), 'assertion')
        assert core.permits(before, after, (violation,))
        assert not core.permits(before, {**after, 'untouched': 1}, (violation,))
        assert not core.Allowed('enum', [1]).accepts(True)
        with pytest.raises(ValueError, match='renderable repair'):
            core.register(replace(row, repair=lambda facts: None), {'reply': before}, 'assertion')
        with pytest.raises(ValueError, match='own witness'):
            core.register(replace(row, check=lambda facts: ({},)), {'reply': before}, 'assertion')
        with pytest.raises(ValueError, match='own witness'):
            core.register(replace(row, check=lambda facts: () if facts['reply']['observer'] == 'assertion'
                                  else ({'bad_value': facts['reply']['observer']},)),
                          {'reply': before}, 'assertion')
        gate = replace(row, gating=True, repair=lambda facts: core.CantMeet('identity differs'))
        core.register(gate, {'reply': before})
        assert 'Further checks were not run' in core.render_all(core.evaluate((gate, row), {'reply': before}))
    finally:
        del sys.modules[spec.name]
