import argparse
import importlib.util
from pathlib import Path
from types import SimpleNamespace

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
    monkeypatch.setattr(argparse.ArgumentParser, 'parse_args', lambda _: SimpleNamespace(
        bundle=bundle, image=image, files=['test_second.py', 'test_first.py'], per_file=per_file))
    calls = []

    def run_checks(repo, base, head, checks, *, container):
        assert base == head
        assert container.image == image
        calls.append(tuple(check.tests for check in checks))
        return ()

    monkeypatch.setattr(inventory, 'run_checks', run_checks)
    assert inventory.main() == 0
    first, second = ('test_first.py::test_ok',), ('test_second.py::test_ok',)
    assert calls == [(first,), (second,)] if per_file else calls == [(first, second)]
