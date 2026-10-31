import json
from pathlib import Path
import subprocess
import sys

import pytest

from tl3_harness import execution
from tl3_harness.model import Check, Kind, Outcome, Verdict
from tl3_harness.report import decode, parse
from tl3_harness.trees import added_names, git

RUNNER = Path(__file__).parents[1] / 'src/tl3_harness/runner.py'


def save(repo, message):
    git(repo, 'add', '--all')
    git(repo, 'add', '--renormalize', '.')
    git(repo, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
        'commit', '-qm', message)
    return git(repo, 'rev-parse', 'HEAD').decode().strip()


def test_reproducer_outside_config_fails_the_round(tmp_path, monkeypatch):
    repo = tmp_path / 'repo'
    repo.mkdir()
    git(repo, 'init', '-q')
    (repo / 'pyproject.toml').write_text(
        '[tool.tl3.harness]\ncommand=["python", "-m", "pytest"]\ntest-files=["tests/*.py"]\n')
    (repo / 'product.py').write_text('def value(): return 1\n')
    reviewed = save(repo, 'reviewed')
    (repo / 'product.py').write_text('def value(): return 2\n')
    (repo / 'outside.py').write_text('def test_value(): assert False\n')
    fixed = save(repo, 'fixed')
    monkeypatch.setattr(execution, 'require_admission', lambda: None)
    monkeypatch.setattr(execution, 'run_tree', lambda *a, **kw: pytest.fail('invalid selection ran'))
    result, = execution.run_checks(repo, reviewed, fixed,
        (Check('reproducer', Kind.REPRODUCING, ('outside.py::test_value',)),),
        reviewed=reviewed, container=execution.Container('sha256:' + '1' * 64))
    assert result.verdict == Verdict.FAIL
    assert 'configured test files' in result.tests[0].message


@pytest.mark.parametrize('statement', ['from product import helper', 'import new_module',
                                      'from package import helper', 'from package.child import helper'])
@pytest.mark.parametrize('collection', [False, True])
@pytest.mark.parametrize('kind', [Kind.REPRODUCING, Kind.DEMONSTRATING])
def test_import_of_added_name_demonstrates_the_round(tmp_path, statement, collection, kind):
    before, after = tmp_path / 'before', tmp_path / 'after'
    before.mkdir()
    after.mkdir()
    for tree in (before, after):
        (tree / 'product.py').write_text('def value(): return 1\n')
        (tree / 'package').mkdir()
        (tree / 'package/__init__.py').write_text('')
        (tree / 'package/child.py').write_text('')
    (after / 'product.py').write_text('def value(): return 1\ndef helper(): return 2\n')
    (after / 'new_module.py').write_text('def helper(): return 2\n')
    (after / 'package/__init__.py').write_text('def helper(): return 2\n')
    (after / 'package/child.py').write_text('def helper(): return 2\n')
    code = (f'{statement}\ndef test_import(): assert True\n' if collection else
            f'def test_import():\n    {statement}\n    assert True\n')
    tests = ('test_import.py::test_import', 'test_other.py::test_other')
    observations = []
    additions = added_names(before, after, ['test_*.py'])
    for tree in (before, after):
        (tree / 'test_import.py').write_text(code)
        (tree / 'test_other.py').write_text('def test_other(): assert True\n')
        child = subprocess.run([sys.executable, '-I', '-B', str(RUNNER), json.dumps(tests), str(tree)],
                               capture_output=True)
        observations.append(parse([decode(line) for line in child.stdout.splitlines()], tests,
                                  set(), child.returncode, additions if tree == before else frozenset()))
    assert observations[0][tests[0]] == Outcome.ADDED_NAME
    assert observations[0][tests[1]] == Outcome.PASSED, 'collection errors belong to their own file'
    from tl3_harness.model import judge
    assert judge(Check('import', kind, (tests[0],)), *observations).verdict == Verdict.PASS


@pytest.mark.parametrize('code', [
    'from product import absent\ndef test_import(): assert True\n',
    'import absent_module\ndef test_import(): assert True\n',
    'raise RuntimeError("collection failure")\ndef test_import(): assert True\n',
    'import pytest\n@pytest.fixture\ndef setup():\n    from product import helper\ndef test_import(setup): assert True\n',
])
def test_unrelated_imports_and_setup_errors_do_not_demonstrate(tmp_path, code):
    (tmp_path / 'product.py').write_text('')
    (tmp_path / 'test_import.py').write_text(code)
    (tmp_path / 'test_other.py').write_text('def test_other(): assert True\n')
    tests = ('test_import.py::test_import', 'test_other.py::test_other')
    child = subprocess.run([sys.executable, '-I', '-B', str(RUNNER), json.dumps(tests), str(tmp_path)],
                           capture_output=True)
    outcomes = parse([decode(line) for line in child.stdout.splitlines()], tests, set(), child.returncode,
                     frozenset({('product.py', 'helper')}))
    assert outcomes[tests[0]] == Outcome.ERROR
    assert outcomes[tests[1]] == Outcome.PASSED


@pytest.mark.parametrize('collection', [False, True])
@pytest.mark.parametrize(('files', 'added_file', 'added_name'), [
    ({'broken.py': 'from other import typo\n', 'other.py': ''}, 'other.py', 'extra'),
    ({'broken.py': 'from other import typo\n', 'other.py': ''}, 'broken.py', 'typo'),
    ({'broken.py': '', 'other.py': ''}, 'other.py', 'extra'),
])
def test_import_error_must_name_the_added_symbol_in_its_own_module(
        tmp_path, collection, files, added_file, added_name):
    before, after = tmp_path / 'before', tmp_path / 'after'
    for tree in (before, after):
        tree.mkdir()
        for name, content in files.items():
            (tree / name).write_text(content)
    with (after / added_file).open('a') as target:
        target.write(f'def {added_name}(): return 2\n')
    code = ('from broken import extra\ndef test_import(): assert True\n' if collection else
            'def test_import():\n    from broken import extra\n    assert True\n')
    (before / 'test_import.py').write_text(code)
    tests = ('test_import.py::test_import',)
    child = subprocess.run([sys.executable, '-I', '-B', str(RUNNER), json.dumps(tests), str(before)],
                           capture_output=True)
    outcomes = parse([decode(line) for line in child.stdout.splitlines()], tests, set(),
                     child.returncode, added_names(before, after, ['test_*.py']))
    assert outcomes[tests[0]] == Outcome.ERROR


@pytest.mark.parametrize('collection', [False, True])
@pytest.mark.parametrize(('source_root', 'statement', 'new_path', 'content'), [
    ('', 'from pkg import newmod', 'pkg/newmod.py', ''),
    ('', 'from pkg import newmod', 'pkg/newmod/__init__.py', ''),
    ('src', 'import srcmod', 'srcmod.py', ''),
    ('workers/src', 'from pkg.sub import name', 'pkg/sub.py', 'def name(): return 2\n'),
])
def test_added_imports_follow_the_active_source_roots(
        tmp_path, collection, source_root, statement, new_path, content):
    before, after = tmp_path / 'before', tmp_path / 'after'
    code = (f'{statement}\ndef test_import(): assert True\n' if collection else
            f'def test_import():\n    {statement}\n    assert True\n')
    for tree in (before, after):
        product = tree / source_root
        (product / 'pkg').mkdir(parents=True)
        (product / 'pkg/__init__.py').write_text('')
        (tree / 'pyproject.toml').write_text(
            f'[tool.pytest.ini_options]\npythonpath = ["{source_root or "."}"]\n')
        (tree / 'test_import.py').write_text(code)
    added = after / source_root / new_path
    added.parent.mkdir(parents=True, exist_ok=True)
    added.write_text(content)
    tests = ('test_import.py::test_import',)
    additions = added_names(before, after, ['test_*.py'])
    for tree, expected in ((before, Outcome.ADDED_NAME), (after, Outcome.PASSED)):
        child = subprocess.run([sys.executable, '-I', '-B', str(RUNNER), json.dumps(tests), str(tree)],
                               capture_output=True)
        outcomes = parse([decode(line) for line in child.stdout.splitlines()], tests, set(),
                         child.returncode, additions)
        assert outcomes[tests[0]] == expected
