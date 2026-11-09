"""Job identity scopes both caches through the complete checks entry point."""
import os

import pytest

from tl3_harness import execution
from tl3_harness.model import Check, Kind, Outcome, Verdict
from tl3_harness.trees import git


@pytest.mark.parametrize('selected', [False, True])
def test_run_checks_without_pb_variables_keeps_both_caches_job_scoped(tmp_path, monkeypatch, selected):
    from tl3_harness import runtime

    for name in tuple(os.environ):
        if name.startswith('PRISMABUILD_'):
            monkeypatch.delenv(name)
    repo = tmp_path / 'checkout'
    repo.mkdir()
    git(repo, 'init', '-q', '-b', 'main')
    (repo / 'pyproject.toml').write_text('[tool.tl3.harness]\ncommand=["python", "-m", "pytest"]\ntest-files=["test_*.py"]\n')
    (repo / 'product.py').write_text('VALUE = 1\n')
    (repo / 'test_planned.py').write_text('def test_planned():\n    pass\n')
    (repo / 'test_extra.py').write_text('def test_extra():\n    pass\n')
    git(repo, 'add', '.')
    git(repo, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'base')
    base = git(repo, 'rev-parse', 'HEAD').decode().strip()
    (repo / 'product.py').write_text('VALUE = 2\n')
    git(repo, 'add', '.')
    git(repo, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'head')
    head = git(repo, 'rev-parse', 'HEAD').decode().strip()
    runs, candidates = [], []

    class Tool(execution.Pytest):
        def select(self, *args):
            return ['test_extra.py']

        def run(self, tree, container, tests, report, progress, *args, **kwargs):
            runs.append((tree.name, tests))
            contract = kwargs.get('selected')
            if contract is not None:
                contract.collected = ('test_extra.py::test_extra',)
            return dict.fromkeys(tests, Outcome.PASSED)

    actual_selected_base = execution.selected_base

    def selected_base(*args, **kwargs):
        candidates.append(args[3])
        return actual_selected_base(*args, **kwargs)

    monkeypatch.setattr(execution, 'require_admission', lambda: None)
    monkeypatch.setattr(execution, 'selected_base', selected_base)
    # The Rust context adapter supplies this internal owner value in production.
    # No invented Rust wire shape is needed to exercise the cache consumers.
    owner = ['flux', 'instance-a', 'job-a']
    monkeypatch.setattr(runtime, 'cache_owner', lambda: tuple(owner), raising=False)
    monkeypatch.setattr(execution, '_BASE_RESULTS', {})
    monkeypatch.setattr(execution, '_SELECTED_BASE', {})
    tool = Tool()
    plan = (Check('planned', Kind.REGRESSION, ('test_planned.py::test_planned',)),)
    image = execution.Container('sha256:' + '1' * 64)

    def round_checks():
        results = execution.run_checks(repo, base, head, plan, container=image, tool=tool,
                                       selected_tests=10 if selected else None)
        assert all(result.verdict == Verdict.PASS for result in results)
        return len([entry for entry in runs if entry[0] == 'base'])

    first = round_checks()
    expected = (('test_extra.py',),) if selected else ()
    assert candidates[-1] == expected
    assert first == (2 if selected else 1)
    assert round_checks() == first, 'same job should reuse both complete base observations'
    owner[-1] = 'job-b'
    assert round_checks() == first * 2, 'another job must observe both bases again'
    owner[1] = 'instance-b'
    assert round_checks() == first * 3, 'same native job id in another instance is a distinct owner'
    assert len(execution._BASE_RESULTS) == 3
    assert len(execution._SELECTED_BASE) == (3 if selected else 0)
