import json
from pathlib import Path
import subprocess
import sys

import pytest

from tl3_harness import execution
from tl3_harness.model import Check, Kind, Outcome, Verdict
from tl3_harness.report import decode, parse
from tl3_harness.trees import git

SOURCE = (Path(__file__).parent / 'fixtures/chunked_test.py').read_text()
TEST = 'tests/test_sequences.py::test_chunked_rejects_non_positive_sizes'
RUNNER = Path(__file__).parents[1] / 'src/tl3_harness/runner.py'


def run(tree, container, tests, report, progress, additions=frozenset(), *, messages=None):
    child = subprocess.run([sys.executable, '-I', '-B', str(RUNNER), json.dumps(tests), str(tree)],
                           capture_output=True)
    return parse([decode(line) for line in child.stdout.splitlines()], tests, set(),
                 child.returncode, additions, messages=messages)


def save(repo, message):
    git(repo, 'add', '--all')
    git(repo, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
        'commit', '-qm', message)
    return git(repo, 'rev-parse', 'HEAD').decode().strip()


@pytest.fixture
def chunked_repo(tmp_path, monkeypatch):
    repo = tmp_path / 'repo'
    (repo / 'tests').mkdir(parents=True)
    git(repo, 'init', '-q')
    (repo / 'pyproject.toml').write_text(
        '[tool.tl3.harness]\ncommand=["python", "-m", "pytest"]\ntest-files=["tests/*.py"]\n')
    (repo / 'tests/test_sequences.py').write_text(SOURCE)
    (repo / 'sequences.py').write_text(
        'def chunked(items, size):\n    if size <= 0: raise ValueError(size)\n    return [items]\n')
    base = save(repo, 'base')
    monkeypatch.setattr(execution, 'require_admission', lambda: None)
    monkeypatch.setattr(execution, 'run_tree', run)
    monkeypatch.setenv('PRISMABUILD_CONTAINER_OWNER', str(repo))
    return repo, base


@pytest.mark.parametrize('broken', [False, True])
def test_parametrized_bare_id_aggregates_base_and_head(chunked_repo, broken):
    repo, base = chunked_repo
    (repo / 'sequences.py').write_text(
        'def chunked(items, size):\n    if size ' + ('< 0' if broken else '<= 0') +
        ': raise ValueError(size)\n    return [items]\n# changed product\n')
    head = save(repo, 'head')
    result, = execution.run_checks(repo, base, head,
        (Check('sizes', Kind.REGRESSION, (TEST,)),),
        container=execution.Container('sha256:' + '1' * 64))
    assert result.tests[0].base == Outcome.PASSED
    assert result.tests[0].head == (Outcome.ASSERTION if broken else Outcome.PASSED)
    assert result.verdict == (Verdict.FAIL if broken else Verdict.PASS)
    if broken:
        assert 'DID NOT RAISE' in result.tests[0].message


@pytest.mark.parametrize('kind', [Kind.REGRESSION, Kind.BEHAVIOR_PRESERVING])
@pytest.mark.parametrize('edit', ['loop', 'delete', 'decorator', 'assertion'])
def test_existing_named_test_is_protected(chunked_repo, kind, edit):
    repo, base = chunked_repo
    path = repo / 'tests/test_sequences.py'
    changes = {
        'loop': 'def test_chunked_rejects_non_positive_sizes():\n    for size in [0, -1]:\n        assert size <= 0\n',
        'delete': 'import pytest\n',
        'decorator': SOURCE.replace('[0, -1]', '[0]'),
        'assertion': SOURCE.replace('list(chunked([1, 2], size))', 'raise ValueError(size)'),
    }
    path.write_text(changes[edit])
    head = save(repo, 'head')
    result, = execution.run_checks(repo, base, head, (Check('sizes', kind, (TEST,)),),
        container=execution.Container('sha256:' + '1' * 64))
    assert result.verdict == Verdict.FAIL
    assert result.tests[0].message == (
        f'Restore the existing test `{TEST}`; existing tests may not be changed to satisfy a check.')
