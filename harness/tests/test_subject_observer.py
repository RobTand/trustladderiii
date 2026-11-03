"""Replay the proof-unit failures recorded in TL3 issue #645's review corpus."""
import json
from pathlib import Path
import sys

import pytest

from tl3_harness import execution, model
from tl3_harness.execution import Container, Progress, run_checks
from tl3_harness.model import Check, Forcing, Kind, Outcome, Verdict
from tl3_harness.trees import git


RUNNER = Path(__file__).parents[1] / 'src/tl3_harness/runner.py'
IMAGE = 'sha256:' + '1' * 64
NODE = 'tests/test_queue.py::test_diagnostic'
CORPUS = {item['request']: item for item in json.loads(
    (Path(__file__).parent / 'fixtures/subject_observer/corpus.json').read_text())}
BASE = '''from types import SimpleNamespace

class Queue:
    def __init__(self, root):
        self.path = root / 'claimed'
        self.path.write_text('mover-key')
        self.records = self
        self.cleaned = False
    def item(self, state, key):
        assert state == 'CLAIMED'
        return {'key': self.path.read_text()} if self.path.exists() else None
    def execute(self, row, timeout_s):
        assert row == {'key': 'mover-key'}
        assert timeout_s == 240
        self.path.unlink()
        self.cleaned = True
        return {'returncode': 7, 'stdout': 'stdout-marker', 'stderr': 'stderr-cause'}

def _execute_mover(q, mover):
    row = q.records.item('CLAIMED', mover)
    assert isinstance(row, dict), 'missing claimed row'
    outcome = q.execute(row, timeout_s=240)
    assert outcome['returncode'] == 0, 'mover failed'
    return outcome

def test_control(tmp_path):
    q = Queue(tmp_path)
    try:
        _execute_mover(q, 'mover-key')
    except AssertionError:
        assert q.cleaned
    else:
        raise AssertionError('a failed mover must still fail')
'''
HEAD = BASE.replace("'mover failed'", "f\"{mover} code={outcome['returncode']} stdout: {outcome['stdout']} stderr: {outcome['stderr']}\"")
OBSERVER = '''
def test_diagnostic(tmp_path):
    import pytest
    q = Queue(tmp_path)
    with pytest.raises(AssertionError) as caught:
        _execute_mover(q, 'mover-key')
    assert 'mover-key code=7 stdout: stdout-marker stderr: stderr-cause' in str(caught.value), 'diagnostic lacks failed outcome'
    assert q.cleaned
'''


@pytest.fixture
def repository(tmp_path, monkeypatch):
    repo = tmp_path / 'served'
    repo.mkdir()
    (repo / 'pyproject.toml').write_text(
        '[tool.tl3.harness]\ncommand=["python", "-m", "pytest"]\ntest-files=["tests/*.py"]\n')
    git(repo, 'init', '-q')
    monkeypatch.setattr(execution, 'require_admission', lambda: None)
    monkeypatch.setenv('PRISMABUILD_CONTAINER_OWNER', str(repo))
    monkeypatch.setattr(Progress, 'commit', lambda *_: None)
    monkeypatch.setattr(execution, 'container_command', lambda tree, container, tests: [
        sys.executable, '-I', '-B', str(RUNNER), '-', str(tree)])

    def save(files):
        for name, source in files.items():
            path = repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(source)
        git(repo, 'add', '.')
        git(repo, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
            'commit', '--allow-empty', '-qm', 'replay')
        return git(repo, 'rev-parse', 'HEAD').decode().strip()
    return repo, save


def round_result(repository, before, after, check, *, controls=()):
    repo, save = repository
    base = save(before)
    head = save(after)
    results = run_checks(repo, base, head, (check, *controls), container=Container(IMAGE))
    return results, base, head


@pytest.mark.parametrize('case_id', [178, 195, 202, 206], ids=lambda n: f'request-{n}')
def test_claimed_queue_diagnostics_keep_the_old_subject(repository, case_id):
    # Saved traces 0f0469e0, 0b98dbfd, 68436d6f, 27f7f232 differ in which
    # literal/setup/fixture driver was deployed. None can compose the claimed row.
    assert CORPUS[case_id]['trace_sha256']
    source = BASE
    observer = OBSERVER
    if case_id in (202, 206):
        source += '\nclass World:\n    def __init__(self, root): self.q = Queue(root)\n'
        observer = observer.replace('q = Queue(tmp_path)', 'q = World(tmp_path).q')
    head = source.replace(BASE, HEAD) + observer
    controls = (Check('unchanged failure invariant', Kind.REGRESSION, ('tests/test_queue.py::test_control',)),)
    results, _, _ = round_result(repository, {'tests/test_queue.py': source},
                                {'tests/test_queue.py': head}, Check('diagnostic', Kind.DEMONSTRATING, (NODE,), proof=model.Proof((), (), 'diagnostic lacks failed outcome')),
                                controls=controls)
    assert [result.verdict for result in results] == [Verdict.PASS, Verdict.PASS], results
    assert (results[0].tests[0].base, results[0].tests[0].head) == (Outcome.ASSERTION, Outcome.PASSED)
    receipt = json.loads(execution.result_json(results))[0]['proof']
    assert receipt['observer_sha256'][NODE]
    assert receipt['base_tree_sha256'] != receipt['head_tree_sha256']


def test_request216_decorated_cross_module_subject_and_existing_observer(repository):
    before = {'tests/test_world.py': 'from contextlib import contextmanager\n@contextmanager\ndef reader_process():\n    yield {"phase": "old"}\n',
              'tests/test_recovery.py': 'from test_world import reader_process\ndef test_recovery():\n    with reader_process() as item:\n        assert item["phase"] == "old", "wrong context phase"\n'}
    after = {'tests/test_world.py': before['tests/test_world.py'].replace('"old"', '"new"'),
             'tests/test_recovery.py': before['tests/test_recovery.py'].replace('"old"', '"new"')}
    results, _, _ = round_result(repository, before, after,
                                Check('context manager', Kind.DEMONSTRATING, ('tests/test_recovery.py::test_recovery',), proof=model.Proof((), (), 'wrong context phase')))
    assert results[0].verdict == Verdict.PASS, results


def test_request233_new_deterministic_git_helper_observer(repository):
    # The source requires a NEW deterministic assertion. A 0/10 old-node
    # FileNotFoundError campaign cannot replace this proof (tr-df2fefe55dec).
    before = {'tests/test_git.py': 'def _git(path):\n    return path.read_text()\n'}
    after = {'tests/test_git.py': '''def _git(path):
    try:
        return path.read_text()
    except FileNotFoundError:
        return 'missing'

def test_missing_output(tmp_path):
    try:
        result = _git(tmp_path / 'gone')
    except FileNotFoundError:
        result = 'unhandled'
    assert result == 'missing', 'missing output must be handled'
'''}
    results, _, _ = round_result(repository, before, after,
                                Check('deterministic helper', Kind.DEMONSTRATING, ('tests/test_git.py::test_missing_output',), proof=model.Proof((), (), 'missing output must be handled')))
    assert results[0].verdict == Verdict.PASS, results


def test_needed_observer_imports_are_derived_without_carrying_the_fix(repository):
    results, _, _ = round_result(repository, {'tests/test_queue.py': BASE},
        {'tests/test_queue.py': 'import pytest\n' + HEAD + OBSERVER.replace('    import pytest\n', '')},
        Check('automatic imports', Kind.DEMONSTRATING, (NODE,), proof=model.Proof((), (), 'diagnostic lacks failed outcome')))
    assert results[0].verdict == Verdict.PASS, results


def test_explicit_partition_rejects_a_subject_fix_in_observer_support(repository):
    proof_type = getattr(model, 'Proof', None)
    assert proof_type is not None, 'one subject/observer source contract must replace HelperEvidence'
    proof = proof_type.parse({'subjects': ['tests/test_queue.py::_execute_mover'],
                              'observer_support': ['tests/test_queue.py::_execute_mover'],
                              'red_message': 'diagnostic lacks failed outcome'})
    check = Check('smuggled fix', Kind.DEMONSTRATING, (NODE,), proof=proof)
    results, _, _ = round_result(repository, {'tests/test_queue.py': BASE},
                                {'tests/test_queue.py': HEAD + OBSERVER}, check)
    assert results[0].verdict == Verdict.FAIL
    assert 'subject' in results[0].tests[0].message.lower()


def test_red_for_another_assertion_is_not_the_declared_reason(repository):
    proof_type = getattr(model, 'Proof', None)
    assert proof_type is not None, 'checks must bind the declared assertion reason'
    proof = proof_type.parse({'subjects': ['tests/test_queue.py::_execute_mover'],
                              'observer_support': [], 'red_message': 'diagnostic lacks failed outcome'})
    observer = OBSERVER.replace("    assert 'mover-key", "    assert not str(caught.value).startswith('mover failed'), 'unrelated assertion'\n    assert 'mover-key")
    results, _, _ = round_result(repository, {'tests/test_queue.py': BASE},
                                {'tests/test_queue.py': HEAD + observer},
                                Check('wrong reason', Kind.DEMONSTRATING, (NODE,), proof=proof))
    assert results[0].verdict == Verdict.FAIL
    assert 'declared' in results[0].tests[0].message.lower()


def test_missing_observer_symbol_names_the_module_and_symbol(repository):
    results, _, _ = round_result(repository, {'tests/test_queue.py': BASE},
        {'tests/test_queue.py': HEAD + OBSERVER.replace('q = Queue(tmp_path)', 'q = new_world(tmp_path)')},
        Check('missing support', Kind.DEMONSTRATING, (NODE,)))
    assert results[0].verdict == Verdict.FAIL
    assert 'new_world' in results[0].tests[0].message
    assert 'tests/test_queue.py' in results[0].tests[0].message


def test_request234_forcing_shadows_only_the_repositorys_local_clock(repository):
    node = 'tests/test_clock.py::test_hold'
    before = {'pool.py': 'import time\ndef held(): return time.monotonic()\n',
              'tests/test_clock.py': 'import pool\ndef test_hold():\n    assert pool.held() > 100, "hold clock did not advance"\n'}
    after = {'tests/test_clock.py': '''import pool
import time

def test_hold(monkeypatch):
    shared = time.monotonic
    monkeypatch.setattr(pool, 'held', lambda: 101)
    assert pool.held() > 100
    assert time.monotonic is shared
    assert time.monotonic() != 100
'''}
    forcing = Forcing.parse({'symptom': 'module-local hold clock is frozen',
                             'patches': [{'target': 'pool.time.monotonic', 'returns': 100}]})
    results, _, _ = round_result(repository, before, after,
                                Check('local clock', Kind.DEMONSTRATING, (node,), forcing, proof=model.Proof((), (), 'hold clock did not advance')))
    assert results[0].verdict == Verdict.PASS, results


@pytest.mark.parametrize('target', ['time.monotonic', 'pool.time.monotonic'])
def test_global_clock_and_external_repository_reference_remain_refused(repository, target):
    # A local alias is allowed only through a repository-owned attachment point.
    node = 'tests/test_external.py::test_external'
    before = {'pool.py': 'import sys\nimport time\nsys.modules[__name__] = time\n',
              'tests/test_external.py': 'def test_external():\n    assert True\n'}
    forcing = Forcing.parse({'symptom': 'invalid global clock', 'patches': [{'target': target, 'returns': 0}]})
    results, _, _ = round_result(repository, before, {},
                                Check('global clock refused', Kind.DEMONSTRATING, (node,), forcing))
    assert results[0].verdict == Verdict.FAIL
    assert 'forcing' in results[0].tests[0].message.lower()


def test_projection_cannot_name_an_outside_repository_subject():
    proof_type = getattr(model, 'Proof', None)
    assert proof_type is not None, 'the proof declaration validates source identities at its boundary'
    with pytest.raises(model.HarnessInputError, match='relative'):
        proof_type.parse({'subjects': ['../outside.py::helper'], 'observer_support': [], 'red_message': 'expected'})


def test_new_fixture_support_uses_real_pytest_context(repository):
    proof = model.Proof(('tests/test_queue.py::_execute_mover',),
                        ('tests/test_queue.py::world',), 'diagnostic lacks failed outcome')
    support = '\nimport pytest\n@pytest.fixture\ndef world(tmp_path):\n    return Queue(tmp_path)\n'
    observer = OBSERVER.replace('test_diagnostic(tmp_path)', 'test_diagnostic(world)').replace('q = Queue(tmp_path)', 'q = world')
    results, _, _ = round_result(repository, {'tests/test_queue.py': BASE},
        {'tests/test_queue.py': HEAD + support + observer}, Check('fixture observer', Kind.DEMONSTRATING, (NODE,), proof=proof))
    assert results[0].verdict == Verdict.PASS, results
    assert results[0].proof['support_sha256']['tests/test_queue.py::world']


def test_existing_class_keeps_subject_and_accepts_new_observer_method(repository):
    before = 'class TestWorld:\n    def value(self): return "old"\n'
    after = before.replace('"old"', '"new"') + '\n    def test_value(self):\n        assert self.value() == "new", "old world"\n'
    proof = model.Proof(('tests/test_world.py::TestWorld::value',), (), 'old world')
    results, _, _ = round_result(repository, {'tests/test_world.py': before}, {'tests/test_world.py': after},
        Check('class observer', Kind.DEMONSTRATING, ('tests/test_world.py::TestWorld::test_value',), proof=proof))
    assert results[0].verdict == Verdict.PASS, results


def test_explicit_fixture_subject_keeps_base_conftest(repository):
    before = {'tests/conftest.py': 'import pytest\n@pytest.fixture\ndef value(): return "old"\n',
              'tests/test_fixture.py': 'def test_value(value): assert value == "old"\n'}
    after = {name: source.replace('"old"', '"new"') for name, source in before.items()}
    after['tests/test_fixture.py'] = 'def test_value(value):\n    assert value == "new", "old fixture"\n'
    proof = model.Proof(('tests/conftest.py::value',), (), 'old fixture')
    results, _, _ = round_result(repository, before, after,
        Check('fixture subject', Kind.DEMONSTRATING, ('tests/test_fixture.py::test_value',), proof=proof))
    assert results[0].verdict == Verdict.PASS, results


@pytest.mark.parametrize('container', ['module', 'class'])
def test_new_observer_context_cannot_smuggle_a_declared_subject(repository, container):
    if container == 'module':
        before = {}
        after = {'tests/test_new.py': 'def subject(): return "fixed"\ndef test_value(): assert subject() == "fixed"\n'}
        subject, observer = 'tests/test_new.py::subject', 'tests/test_new.py::test_value'
    else:
        before = {'tests/test_new.py': 'def control(): pass\n'}
        after = {'tests/test_new.py': 'class TestWorld:\n    def subject(self): return "fixed"\n    def test_value(self): assert self.subject() == "fixed"\n'}
        subject, observer = 'tests/test_new.py::TestWorld::subject', 'tests/test_new.py::TestWorld::test_value'
    proof = model.Proof((subject,), (), 'old subject')
    results, _, _ = round_result(repository, before, after,
        Check('smuggled context', Kind.DEMONSTRATING, (observer,), proof=proof))
    assert results[0].verdict == Verdict.FAIL
    assert 'subject onto base' in results[0].tests[0].message


def test_new_observer_class_carries_its_needed_decorator_import(repository):
    before = 'def helper(): return "old"\n'
    after = (before.replace('"old"', '"new"') + 'import pytest\n'
             '@pytest.mark.parametrize("expected", ["new"])\n'
             'class TestWorld:\n    def test_value(self, expected):\n'
             '        assert helper() == expected, "old world"\n')
    proof = model.Proof(('tests/test_world.py::helper',), (), 'old world')
    results, _, _ = round_result(repository, {'tests/test_world.py': before}, {'tests/test_world.py': after},
        Check('class context', Kind.DEMONSTRATING, ('tests/test_world.py::TestWorld::test_value',), proof=proof))
    assert results[0].verdict == Verdict.PASS, results


@pytest.mark.parametrize('support', [
    'TOKEN = helper = lambda: ("new", VERSION)',
    'TOKEN = "context"; helper = lambda: ("new", VERSION)',
    '(TOKEN, helper) = ("context", lambda: ("new", VERSION))',
    'TOKEN = "context"; globals()["helper"] = lambda: ("new", VERSION)',
    'TOKEN = "context"; globals().update(helper=lambda: ("new", VERSION))',
    'TOKEN = "context"; helper += ("new", VERSION)',
    'TOKEN = "context"; import builtins as helper',
    'TOKEN = "context"; globals().update(**{"helper": lambda: ("new", VERSION)})',
    'def TOKEN():\n    global helper\n    def helper(): return ("new", VERSION)',
    'class TOKEN:\n    global helper\n    class helper: pass',
])
def test_review662_support_cannot_rebind_base_subject(repository, support):
    path = 'tests/test_subject.py'
    before = 'VERSION = 0\ndef helper(): return ("old", VERSION)\n'
    after = ('VERSION = 1\n' + support + '\n'
             'def test_observer():\n    assert helper() == ("new", 1), "declared result"\n')
    proof = model.Proof((path + '::helper',), (path + '::TOKEN',), 'declared result')
    results, _, _ = round_result(repository, {path: before}, {path: after},
        Check('support rebind', Kind.DEMONSTRATING, (path + '::test_observer',), proof=proof))
    assert results[0].verdict == Verdict.FAIL, results
    assert 'bind' in results[0].tests[0].message, results


def test_review662_inferred_proof_cannot_skip_assertion_reason(repository):
    path = 'tests/test_subject.py'
    before = 'EXPECTED = False\ndef helper(): return "old"\n'
    after = ('EXPECTED = True\ndef helper(): return "new"\n'
             'def test_observer():\n    assert EXPECTED, "unrelated assertion"\n'
             '    assert helper() == "new", "declared result"\n')
    results, _, _ = round_result(repository, {path: before}, {path: after},
        Check('unbound reason', Kind.DEMONSTRATING, (path + '::test_observer',)))
    assert results[0].verdict == Verdict.FAIL, results
    assert 'assertion reason' in results[0].tests[0].message, results


@pytest.mark.parametrize('support', [
    'TOKEN = globals(); TOKEN["helper"] = lambda: ("new", VERSION)',
    'TOKEN = "context"; globals().__ior__({"helper": lambda: ("new", VERSION)})',
    'TOKEN = setattr(__import__("sys").modules[__name__], "helper", lambda: ("new", VERSION))',
    'TOKEN = globals()',
    'TOKEN = locals()',
    'TOKEN = vars()',
    'TOKEN = eval("0")',
    'TOKEN = exec("pass")',
    'TOKEN = __import__("builtins")',
    'import builtins as TOKEN',
    'import sys as TOKEN',
])
def test_rereview662_support_refuses_dynamic_namespace_apis(repository, support):
    path = 'tests/test_subject.py'
    before = 'VERSION = 0\ndef helper(): return ("old", VERSION)\n'
    after = (before.replace('= 0', '= 1').replace('"old"', '"new"') + support + '\n'
             'def test_observer():\n    assert helper() == ("new", 1), "declared result"\n')
    proof = model.Proof((path + '::helper',), (path + '::TOKEN',), 'declared result')
    results, _, _ = round_result(repository, {path: before}, {path: after},
        Check('namespace escape', Kind.DEMONSTRATING, (path + '::test_observer',), proof=proof))
    assert results[0].verdict == Verdict.FAIL, results
    assert 'dynamic namespace' in results[0].tests[0].message, results
    assert path in results[0].tests[0].message, results


@pytest.mark.parametrize('deferred', [False, True], ids=['module-support', 'fixture-support'])
def test_rereview662_runtime_rejects_indirect_subject_rebinding(repository, deferred):
    path = 'tests/test_subject.py'
    before = ('VERSION = 0\ndef helper(): return ("old", VERSION)\n'
              'def install(value):\n    globals()["helper"] = value\n')
    support = ('import pytest\n@pytest.fixture(autouse=True)\ndef TOKEN():\n'
               '    install(lambda: ("new", VERSION))\n' if deferred else
               'TOKEN = install(lambda: ("new", VERSION))\n')
    after = (before.replace('VERSION = 0', 'VERSION = 1').replace('return ("old"', 'return ("new"')
             + support + 'def test_observer():\n'
             '    assert helper() == ("new", 1), "declared result"\n')
    proof = model.Proof((path + '::helper',), (path + '::TOKEN',), 'declared result')
    results, _, _ = round_result(repository, {path: before}, {path: after},
        Check('indirect rebind', Kind.DEMONSTRATING, (path + '::test_observer',), proof=proof))
    assert results[0].verdict == Verdict.FAIL, results
    assert 'proof_subject_integrity' in results[0].tests[0].message, results
    assert path in results[0].tests[0].message and 'helper' in results[0].tests[0].message, results
