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


@pytest.mark.parametrize('kind', [Kind.REGRESSION, Kind.DEMONSTRATING])
def test_request204_head_only_inventory_observer_cannot_repair_original_base_regression(repository, kind):
    path = 'tests/test_bazel_inputs.py'
    node = path + '::test_a_passing_file_records_its_inventory'
    original = 'from inventory import recorded\ndef test_existing_control(): assert isinstance(recorded(), bool)\n'
    observer = 'def test_a_passing_file_records_its_inventory(): assert recorded(), "passing file inventory missing"\n'
    before = {'inventory.py': 'def recorded(): return False\n', path: original}
    after = {'inventory.py': 'def recorded(): return True\n', path: original + observer}
    proof = model.Proof((), (), 'passing file inventory missing') if kind == Kind.DEMONSTRATING else None
    result, base, head = round_result(repository, before, after,
        Check('inventory observer', kind, (node,), proof=proof))
    control, = run_checks(repository[0], base, head,
        (Check('existing inventory control', Kind.REGRESSION, (path + '::test_existing_control',)),),
        container=Container(IMAGE))
    assert control.verdict == Verdict.PASS
    assert (control.tests[0].base, control.tests[0].head) == (Outcome.PASSED, Outcome.PASSED)
    assert result[0].tests[0].head == Outcome.PASSED
    if kind == Kind.REGRESSION:
        assert result[0].verdict == Verdict.FAIL
        assert result[0].tests[0].base == Outcome.UNCOLLECTED
    else:
        assert result[0].verdict == Verdict.PASS
        assert result[0].tests[0].base == Outcome.ASSERTION


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


def test_new_fixture_support_cannot_call_a_base_constructor(repository):
    proof = model.Proof(('tests/test_queue.py::_execute_mover',),
                        ('tests/test_queue.py::world',), 'diagnostic lacks failed outcome')
    support = '\nimport pytest\n@pytest.fixture\ndef world(tmp_path):\n    return Queue(tmp_path)\n'
    observer = OBSERVER.replace('test_diagnostic(tmp_path)', 'test_diagnostic(world)').replace('q = Queue(tmp_path)', 'q = world')
    results, _, _ = round_result(repository, {'tests/test_queue.py': BASE},
        {'tests/test_queue.py': HEAD + support + observer}, Check('fixture observer', Kind.DEMONSTRATING, (NODE,), proof=proof))
    # #842 r2: the imported/base constructor is outside the support call allowlist.
    assert results[0].verdict == Verdict.FAIL, results
    assert 'Proof assembly refused' in results[0].tests[0].message
    assert results[0].tests[0].base == Outcome.UNCOLLECTED


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


def test_rereview662_old_subject_can_update_its_own_runtime_state(repository):
    path = 'tests/test_counter.py'
    before = 'COUNT = 0\ndef helper():\n    global COUNT\n    COUNT += 1\n    return "old"\n'
    after = before.replace('"old"', '"new"') + (
        'def test_observer():\n    assert helper() == "new", "declared result"\n    assert COUNT == 1\n')
    check = Check('stateful subject', Kind.DEMONSTRATING, (path + '::test_observer',),
                  proof=model.Proof((path + '::helper',), (), 'declared result'))
    results, _, _ = round_result(repository, {path: before}, {path: after}, check)
    assert results[0].verdict == Verdict.PASS, results
    assert results[0].tests[0].base == Outcome.ASSERTION, results


def test_rereview662_declared_forcing_may_replace_its_dependency(repository):
    before = {'pool.py': 'def now(): return 0\ndef held(): return now()\n',
              'tests/test_clock.py': 'def test_control(): pass\n'}
    after = {'pool.py': before['pool.py'].replace('return now()', 'return now() + 1'),
             'tests/test_clock.py': 'import pool\ndef test_observer():\n'
             '    assert pool.held() != 100, "clock result stayed at 100"\n'}
    check = Check('forced dependency', Kind.DEMONSTRATING, ('tests/test_clock.py::test_observer',),
                  forcing=Forcing.parse({'symptom': 'clock result stays at 100',
                                         'patches': [{'target': 'pool.now', 'returns': 100}]}),
                  proof=model.Proof(('pool.py::held',), (), 'clock result stayed at 100'))
    results, _, _ = round_result(repository, before, after, check)
    assert results[0].verdict == Verdict.PASS, results
    assert results[0].tests[0].base == Outcome.ASSERTION, results


@pytest.mark.parametrize('actual,declared,fail,expected', [
    ('helper `new` result', 'helper `new` result', False, Verdict.PASS),
    ('helper `new` result', 'helper `new` result', True, Verdict.PASS),
    ('declared result\nsecond line', 'declared result\nsecond line', False, Verdict.PASS),
    ('declared result\nUNDECLARED DETAIL', 'declared result', False, Verdict.FAIL),
    ('declared result\nassert UNDECLARED DETAIL', 'declared result', 'raise', Verdict.FAIL),
    ('declared result\nassert detail', 'declared result\nassert detail', 'raise', Verdict.PASS),
    ('declared result\nassert detail', 'declared result\nassert detail', False, Verdict.PASS),
], ids=['backticks', 'pytest-fail', 'multiline', 'undeclared-line',
        'manual-undeclared-assert-line', 'manual-whole-reason', 'rewritten-whole-reason'])
def test_review662_reason_binding_preserves_the_original_message(repository, actual, declared, fail, expected):
    path = 'tests/test_reason.py'
    before = 'def helper(): return "old"\n'
    observer = (f'    if helper() != "new": raise AssertionError({actual!r})\n' if fail == 'raise' else
                f'    if helper() != "new": pytest.fail({actual!r})\n' if fail else
                f'    assert helper() == "new", {actual!r}\n')
    after = before.replace('"old"', '"new"') + 'import pytest\ndef test_observer():\n' + observer
    check = Check('exact reason', Kind.DEMONSTRATING, (path + '::test_observer',),
                  proof=model.Proof((path + '::helper',), (), declared))
    results, _, _ = round_result(repository, {path: before}, {path: after}, check)
    assert results[0].verdict == expected, results
    assert results[0].tests[0].base == Outcome.ASSERTION, results


@pytest.mark.parametrize('existing', [False, True], ids=['new-module', 'existing-module'])
@pytest.mark.parametrize('observer', [
    'import app\ndef test_observer():\n    assert globals().get("__name__") and app.helper() == "new", "old helper"\n',
    'import importlib\ndef test_observer():\n    assert importlib.import_module("app").helper() == "new", "old helper"\n',
], ids=['globals', 'importlib'])
def test_review662_namespace_apis_remain_available_to_observers(repository, existing, observer):
    path = 'tests/test_observer.py'
    before = {'app.py': 'def helper(): return "old"\n'}
    if existing:
        before[path] = 'def test_control(): pass\n'
    after = {'app.py': before['app.py'].replace('"old"', '"new"'), path: observer}
    check = Check('normal observer', Kind.DEMONSTRATING, (path + '::test_observer',),
                  proof=model.Proof(('app.py::helper',), (), 'old helper'))
    results, _, _ = round_result(repository, before, after, check)
    assert results[0].verdict == Verdict.PASS, results
    assert results[0].tests[0].base == Outcome.ASSERTION, results


@pytest.mark.parametrize('support', [False, True], ids=['base-initialization', 'support-calls-base'])
def test_review662_base_subject_can_update_state_during_import(repository, support):
    path = 'tests/test_counter.py'
    before = 'COUNT = 0\ndef helper():\n    global COUNT\n    COUNT += 1\n    return "old"\n'
    observer = 'def test_observer():\n    assert helper() == "new", "old helper"\n'
    if support:
        after = before.replace('"old"', '"new"') + 'TOKEN = helper()\n' + observer.replace('helper()', 'TOKEN')
    else:
        before += observer + 'helper()\n'
        after = before.replace('return "old"', 'return "new"')
    check = Check('stateful import', Kind.DEMONSTRATING, (path + '::test_observer',),
                  proof=model.Proof((path + '::helper',), (path + '::TOKEN',) if support else (), 'old helper'))
    results, _, _ = round_result(repository, {path: before}, {path: after}, check)
    if support:
        # #842 r2: a support initializer cannot call into the base subject.
        assert results[0].verdict == Verdict.FAIL, results
        assert 'Proof assembly refused' in results[0].tests[0].message
        assert results[0].tests[0].base == Outcome.UNCOLLECTED
    else:
        assert results[0].verdict == Verdict.PASS, results
        assert results[0].tests[0].base == Outcome.ASSERTION, results


@pytest.mark.parametrize('support', [
    'TOKEN = World.helper = lambda self: ("new", VERSION)',
    '(TOKEN, World.helper) = ("context", lambda self: ("new", VERSION))',
    'TOKEN = "context"; World.helper = lambda self: ("new", VERSION)',
    'TOKEN = "context"; World.helper: object = lambda self: ("new", VERSION)',
    'TOKEN = "context"; World.values["value"] = "new"',
    'TOKEN = "context"; World.values["value"] += "new"',
    'TOKEN = "context"; del World.helper',
    'TOKEN = "context"; del World.values["value"]',
], ids=['chained', 'tuple', 'attribute', 'annotated', 'subscript', 'augmented', 'delete-attribute', 'delete-subscript'])
def test_final662_support_cannot_assign_or_delete_members(repository, support):
    path = 'tests/test_members.py'
    before = ('VERSION = 0\nclass World:\n    values = {"value": "old"}\n'
              '    def helper(self): return (self.values["value"], VERSION)\n')
    after = (before.replace('= 0', '= 1').replace('self.values["value"]', '"new"') + support + '\n'
             'def test_observer():\n    assert World().helper() == ("new", 1), "declared result"\n')
    check = Check('member write', Kind.DEMONSTRATING, (path + '::test_observer',),
                  proof=model.Proof((path + '::World::helper',), (path + '::TOKEN',), 'declared result'))
    results, _, _ = round_result(repository, {path: before}, {path: after}, check)
    assert results[0].verdict == Verdict.FAIL, results
    assert 'plain names' in results[0].tests[0].message, results
    assert path in results[0].tests[0].message and 'World' in results[0].tests[0].message, results


def test_final662_observer_can_update_an_unrelated_existing_counter(repository):
    path = 'tests/test_counter.py'
    before = 'VISITS = 0\ndef helper(): return "old"\n'
    after = before.replace('"old"', '"new"') + (
        'def test_observer():\n    global VISITS\n    VISITS += 1\n'
        '    assert helper() == "new", "declared result"\n    assert VISITS == 1\n')
    check = Check('observer bookkeeping', Kind.DEMONSTRATING, (path + '::test_observer',),
                  proof=model.Proof((path + '::helper',), (), 'declared result'))
    results, _, _ = round_result(repository, {path: before}, {path: after}, check)
    assert results[0].verdict == Verdict.PASS, results
    assert results[0].tests[0].base == Outcome.ASSERTION, results


@pytest.mark.parametrize('existing', [False, True], ids=['appended', 'replaced'])
def test_final662_splicing_preserves_crlf_subject_bytes_and_hash(repository, tmp_path, existing):
    import hashlib
    from tl3_harness.proof import assemble_proof
    from tl3_harness.trees import configuration, export

    repo, save = repository
    path = 'tests/test_newlines.py'
    subject = 'def helper(): return "old"\r\n'
    observer = 'def test_observer():\n    assert helper() == "new", "old helper"\n'
    base = save({path: subject + (observer.replace('\n', '\r\n') if existing else '')})
    head = save({path: subject.replace('"old"', '"new"') + observer})
    old, new, staged = (tmp_path / name for name in ('base', 'head', 'staged'))
    export(repo, base, old)
    export(repo, head, new)
    check = Check('newlines', Kind.DEMONSTRATING, (path + '::test_observer',),
                  proof=model.Proof((path + '::helper',), (), 'old helper'))
    receipt = assemble_proof(old, new, staged, check, configuration(old))
    assert (staged / path).read_bytes().startswith(subject.encode())
    assert receipt['subjects'][path + '::helper']['base'] == hashlib.sha256(subject.encode()).hexdigest()


def test_final662_support_wildcard_import_cannot_replace_base_helper(repository):
    path = 'tests/test_subject.py'
    before = 'VERSION = 0\ndef helper(): return "old"\n'
    after = before.replace('= 0', '= 1').replace('"old"', '"new"') + (
        'TOKEN = 1; from support_fix import *\n'
        'def test_observer():\n    assert (helper(), VERSION) == ("new", 1), "declared result"\n')
    check = Check('wildcard support', Kind.DEMONSTRATING, (path + '::test_observer',),
                  proof=model.Proof((path + '::helper',), (path + '::TOKEN', 'support_fix.py'), 'declared result'))
    results, _, _ = round_result(repository, {path: before},
                                {path: after, 'support_fix.py': 'def helper(): return "new"\n'}, check)
    assert results[0].verdict == Verdict.FAIL, results
    assert 'wildcard import' in results[0].tests[0].message, results
    assert path in results[0].tests[0].message and 'support_fix' in results[0].tests[0].message, results


def test_request233_same_file_git_configuration_has_assertion_red(repository):
    # tr-e9f7e102: assertion RED is required; the intermittent race is not RED.
    path = 'tests/test_checkout_recovery_1465.py'
    before = """import os
import subprocess

def _git(root, *arguments):
    return subprocess.run(['git', '-C', str(root), *arguments], check=True,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                          env={**os.environ, 'GIT_CONFIG_GLOBAL': os.devnull,
                               'GIT_CONFIG_NOSYSTEM': '1'}).stdout.strip()
"""
    fixed = before.replace("str(root), *arguments", "str(root), '-c', 'maintenance.auto=false', '-c', 'gc.auto=0', *arguments")
    observer = """
def test_git_disables_maintenance(monkeypatch, tmp_path):
    from types import SimpleNamespace
    calls = []
    def capture(command, **options):
        calls.append((command, options))
        return SimpleNamespace(stdout='ok')
    monkeypatch.setattr(subprocess, 'run', capture)
    assert _git(tmp_path, 'status') == 'ok'
    command, options = calls[0]
    assert ('-c', 'maintenance.auto=false', '-c', 'gc.auto=0') == tuple(command[3:7]), 'automatic maintenance remains enabled'
    assert options['env']['GIT_CONFIG_GLOBAL'] == os.devnull
    assert options['env']['GIT_CONFIG_NOSYSTEM'] == '1'
"""
    proof = model.Proof((path + '::_git',), (), 'automatic maintenance remains enabled')
    results, _, _ = round_result(repository, {path: before}, {path: fixed + observer},
        Check('deterministic git configuration', Kind.DEMONSTRATING,
              (path + '::test_git_disables_maintenance',), proof=proof))
    assert results[0].verdict == Verdict.PASS, results



@pytest.mark.parametrize('case', json.loads(
    (Path(__file__).parent / 'fixtures/subject_observer/ops_1105.json').read_text()),
    ids=lambda case: case['case'])
def test_t842_real_ops_proof_shapes(repository, case):
    # Q27/#645: replay the actual observer and declaration, preserving base subjects.
    check = Check(case['case'], Kind.DEMONSTRATING, tuple(case['tests']),
                  proof=model.Proof.parse(case['proof']))
    results, _, _ = round_result(repository, case['before'], case['after'], check)
    if case['case'] == 'RobTand/prismabuild#2816':
        # Its mutable dictionary escapes into production code. Partial inference
        # cannot prove identical data; planner preflight must request a factory.
        assert results[0].verdict == Verdict.FAIL, results
        assert 'immutable and bound exactly once' in results[0].tests[0].message
        assert results[0].tests[0].base == Outcome.UNCOLLECTED
        return
    # #842 r2: replacing submit.PBRUN is runtime mutation of a loaded subject.
    assert results[0].verdict == Verdict.FAIL, results
    assert 'Proof assembly refused' in results[0].tests[0].message
    assert results[0].tests[0].base == Outcome.UNCOLLECTED


def assemble_t842_case(tmp_path, case):
    from tl3_harness.proof import assemble_proof

    base, head, staged = (tmp_path / name for name in ('base', 'head', 'staged'))
    for root, files in ((base, case['before']), (head, {**case['before'], **case['after']})):
        root.mkdir()
        for name, text in files.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
    check = Check(case['case'], Kind.DEMONSTRATING, tuple(case['tests']),
                  proof=model.Proof.parse(case['proof']))
    receipt = assemble_proof(base, head, staged, check, {'test-files': ['tests/*.py']})
    return receipt, base, head, staged


@pytest.fixture
def t842_cases():
    return json.loads((Path(__file__).parent / 'fixtures/subject_observer/ops_1105.json').read_text())


@pytest.mark.parametrize('extra', [
    'TOKEN = globals()',
    'TOKEN = locals()',
    'TOKEN = vars(sys)',
    'TOKEN = getattr(sys, "modules")',
    'setattr(sys, "modules", {})',
    'delattr(sys, "modules")',
    'TOKEN = eval("1")',
    'exec("subject = 1")',
    'TOKEN = compile("1", "support", "eval")',
    'TOKEN = __import__("subject")',
    'TOKEN = sys.modules',
    'TOKEN = sys.__dict__',
    'TOKEN = sys',
    'from sys import modules',
    'import sys as system',
    'import subject as sys',
    'from subject import namespace as sys',
    'import importlib',
    'from importlib import import_module',
    'from support_fix import *',
    'TOKEN = sys.executable = "fixed"',
    '(TOKEN, sys.executable) = ("context", "fixed")',
    'sys.argv[0] = "fixed"',
    'sys.argv[0] += "fixed"',
    'del sys.argv[0]',
    'del sys.executable',
    'PATCH = mock.patch.object(sys, "argv", [])',
    'with mock.patch.object(sys, "modules", {}): pass',
])
def test_t842_new_loader_context_keeps_namespace_and_write_refusals(tmp_path, t842_cases, extra):
    # Q27/#645: admitting the real loader must not admit arbitrary support writes.
    case = t842_cases[1]
    case['after']['tests/test_pb_duplicate_arms.py'] += '\n' + extra + '\n'
    with pytest.raises(model.ProofRefusal):
        assemble_t842_case(tmp_path, case)


@pytest.mark.parametrize('old,new', [
    ('experiments/graph_attest_702/submit.py', '../head/experiments/graph_attest_702/submit.py'),
    ('spec.loader.exec_module(module)', 'spec.loader.exec_module(sys)'),
    ('module = importlib.util.module_from_spec(spec)', 'module = sys.modules["subject"]'),
    ('    return module', '    module.parse_plan = lambda path: []\n    return module'),
    ('    return module', '    del module.parse_plan\n    return module'),
    ('spec_from_file_location("graph_attest_submit", path)',
     'spec_from_file_location("graph_attest_submit", path, submodule_search_locations=[])'),
    ('from pathlib import Path', 'from pathlib import Path\nPath = lambda _: None'),
    ('from pathlib import Path', 'from .pathlib import Path'),
])
def test_t842_loader_must_execute_a_fresh_module_from_the_declared_subject(tmp_path, t842_cases, old, new):
    # #842: no head-only path, existing module, replacement loader or subject mutation.
    case = t842_cases[1]
    path = 'tests/test_pb_duplicate_arms.py'
    assert old in case['after'][path]
    case['after'][path] = case['after'][path].replace(old, new, 1)
    with pytest.raises(model.ProofRefusal):
        assemble_t842_case(tmp_path, case)


@pytest.mark.parametrize('binding', [
    '_X86_CONFIG = dict(x86_producer={})',
    '_X86_CONFIG = globals()',
    '_X86_CONFIG = pbtest_capabilities.classify_files = {}',
    '_X86_CONFIG, pbtest_capabilities.classify_files = {}, None',
    '_X86_CONFIG = {}; pbtest_capabilities.classify_files = lambda *args: ({}, ["fixed"])',
    '_X86_CONFIG = {}; del pbtest_capabilities.classify_files',
    '_X86_CONFIG = {}; from support_fix import *',
])
def test_t842_inferred_support_is_only_a_single_plain_literal_binding(tmp_path, t842_cases, binding):
    case = t842_cases[0]
    path = 'tests/test_pbtest_capability_cohorts.py'
    source = case['after'][path]
    start = source.index('_X86_CONFIG =')
    end = source.index('@pytest.mark.parametrize', start)
    case['after'][path] = source[:start] + binding + '\n\n' + source[end:]
    with pytest.raises(model.ProofRefusal):
        assemble_t842_case(tmp_path, case)


def test_t842_inference_cannot_graft_a_declared_constant_subject(tmp_path, t842_cases):
    case = t842_cases[0]
    case['proof']['subjects'].append('tests/test_pbtest_capability_cohorts.py::_X86_CONFIG')
    with pytest.raises(model.ProofRefusal, match='rebind a base or subject'):
        assemble_t842_case(tmp_path, case)


def test_t842_inference_preserves_an_existing_constant_subject(tmp_path, t842_cases):
    case = t842_cases[0]
    path = 'tests/test_pbtest_capability_cohorts.py'
    case['before'][path] += '\n_X86_CONFIG = {"base": True}\n'
    case['proof']['subjects'].append(path + '::_X86_CONFIG')
    receipt, base, _, staged = assemble_t842_case(tmp_path, case)
    from tl3_harness.proof import unit

    assert unit(staged, path + '::_X86_CONFIG') == unit(base, path + '::_X86_CONFIG')
    assert path + '::_X86_CONFIG' not in receipt['support_sha256']


def test_t842_inferred_literal_support_is_bound_in_the_receipt(tmp_path):
    import hashlib

    case = t842_immutable_case()
    receipt, base, head, staged = assemble_t842_case(tmp_path, case)
    from tl3_harness.proof import unit

    support = 'tests/test_observer.py::_DATA'
    subject = case['proof']['subjects'][0]
    assert receipt['support_sha256'][support] == hashlib.sha256(unit(head, support).encode()).hexdigest()
    assert unit(staged, support) == unit(head, support)
    assert unit(staged, subject) == unit(base, subject)
    assert receipt['declaration'] == case['proof']


def test_t842_literal_inference_cannot_shadow_a_base_wildcard(tmp_path, t842_cases):
    case = t842_cases[0]
    case['before']['tests/test_pbtest_capability_cohorts.py'] += (
        '\nfrom old_support import *\n'
        'def test_an_unreadable_assigned_file_is_refused_by_name(): pass\n')
    with pytest.raises(model.ProofRefusal, match='wildcard'):
        assemble_t842_case(tmp_path, case)


@pytest.mark.parametrize('extra', [
    '__file__ = "/head/tests/test_pb_duplicate_arms.py"',
    'del __file__',
    'from somewhere import location as __file__',
])
def test_t842_loader_cannot_replace_its_repository_anchor(tmp_path, t842_cases, extra):
    case = t842_cases[1]
    case['after']['tests/test_pb_duplicate_arms.py'] += '\n' + extra + '\n'
    with pytest.raises(model.ProofRefusal, match='dynamic namespace'):
        assemble_t842_case(tmp_path, case)


@pytest.mark.parametrize('source', [
    'import sys\nTOKEN = sys.executable\n',
    'from sys import executable\n',
    'import importlib.util\n',
])
def test_t842_loader_exceptions_do_not_apply_to_unrelated_support(source):
    from tl3_harness.proof import validate_support

    with pytest.raises(model.ProofRefusal, match='dynamic namespace'):
        validate_support(source, 'support.py')


@pytest.mark.parametrize('case', json.loads(
    (Path(__file__).parent / 'fixtures/subject_observer/review_842.json').read_text()),
    ids=lambda case: case['finding'])
def test_t842_review_counterexamples_refuse_before_execution(repository, tmp_path, case):
    # #842 review / #645: assembly must refuse even when a forged proof would pass.
    with pytest.raises(model.ProofRefusal):
        assemble_t842_case(tmp_path, {**case, 'case': case['finding']})
    check = Check(case['finding'], Kind.DEMONSTRATING, tuple(case['tests']),
                  proof=model.Proof.parse(case['proof']))
    results, _, _ = round_result(repository, case['before'], case['after'], check)
    assert results[0].verdict == Verdict.FAIL, results
    assert 'Proof assembly refused' in results[0].tests[0].message, results
    assert results[0].tests[0].base == Outcome.UNCOLLECTED, results


# Python's symbol table, rather than Name(Store), defines these bindings.
T842_BINDING_FORMS = [
    '{name} = None',
    '({name}, (other, *rest)) = (None, (1, 2))',
    '*{name}, other = (1, 2)',
    '{name}: object = None',
    '{name}: object',
    '{name} += 1',
    '({name} := None)',
    'for {name} in (): pass',
    'async def task():\n    async for {name} in stream: pass',
    'with context() as {name}: pass',
    'async def task():\n    async with context() as {name}: pass',
    'try: pass\nexcept Exception as {name}: pass',
    'import other as {name}',
    'from other import value as {name}',
    'def {name}(): pass',
    'async def {name}(): pass',
    'class {name}: pass',
    'match None:\n    case {name}: pass',
    'match []:\n    case [*{name}]: pass',
    'match {{}}:\n    case {{**{name}}}: pass',
    '[None for {name} in ()]',
    '(None for {name} in ())',
    'def task({name}): pass',
    'task = lambda {name}: None',
    'def task():\n    global {name}',
    'def outer():\n    {name} = None\n    def inner():\n        nonlocal {name}',
    'del {name}',
    'type {name} = object',
    'def task[{name}](): pass',
    'class task[{name}]: pass',
    'type task[{name}] = list[{name}]',
]


@pytest.mark.parametrize('name', ['__file__', 'Path', 'sys', 'mock', 'unittest'])
@pytest.mark.parametrize('form', T842_BINDING_FORMS)
def test_t842_loader_inputs_cannot_be_bound_by_any_python_form(tmp_path, t842_cases, name, form):
    case = t842_cases[1]
    path = 'tests/test_pb_duplicate_arms.py'
    if name == 'unittest':
        case['after'][path] = case['after'][path].replace('from unittest import mock', 'import unittest.mock').replace(
            'mock.patch.object', 'unittest.mock.patch.object')
    case['after'][path] += '\n' + form.format(name=name) + '\n'
    with pytest.raises(model.ProofRefusal):
        assemble_t842_case(tmp_path, case)


@pytest.mark.parametrize('form', [
    '{name} = None',
    '({name}, (other, *rest)) = (None, (1, 2))',
    '*{name}, other = (1, 2)',
    '{name}: object = None',
    '{name} += 1',
    '({name} := None)',
    'for {name} in (): pass',
    'with context() as {name}: pass',
    'try: pass\nexcept Exception as {name}: pass',
    'import other as {name}',
    'from other import value as {name}',
    'def {name}(): pass',
    'class {name}: pass',
    'match None:\n    case {name}: pass',
    'match []:\n    case [*{name}]: pass',
    'match {{}}:\n    case {{**{name}}}: pass',
    'del {name}',
])
def test_t842_support_binding_forms_cannot_rebind_subjects(tmp_path, form):
    from textwrap import indent

    before = 'def helper(): return 0\n'
    binding = ('TOKEN = 1; ' + form.format(name='helper') if ': object' in form else
               'def TOKEN():\n    global helper\n' + indent(form.format(name='helper'), '    '))
    source = before.replace('return 0', 'return 1') + '\n' + binding + (
        '\n\ndef test_observer():\n    assert helper() == 1, "bug"\n')
    case = {'case': 'subject writes', 'before': {'tests/test_subject.py': before},
            'after': {'tests/test_subject.py': source}, 'tests': ['tests/test_subject.py::test_observer'],
            'proof': {'subjects': ['tests/test_subject.py::helper'],
                      'observer_support': ['tests/test_subject.py::TOKEN'], 'red_message': 'bug'}}
    with pytest.raises(model.ProofRefusal, match='rebind|ambiguous'):
        assemble_t842_case(tmp_path, case)


@pytest.mark.parametrize('binding', [
    '_DATA = {"expected": 0}\nalias = _DATA\nalias.update(expected=1)',
    '_DATA = {"expected": 0}\nchange(_DATA)',
    '_DATA = ({"expected": 0},)',
    '_DATA = 0\nmatch 1:\n    case _DATA: pass',
    '_DATA = 0\n(_DATA := 1)',
    '_DATA = 0\ndef change():\n    global _DATA\n    _DATA = 1',
    '_DATA = 0\ndel _DATA',
    '_DATA = 0\nglobals()["_DATA"] = 1',
    '_DATA = 0\ndef change():\n    globals().update(_DATA=1)',
    '_DATA = 0\nexec("_DATA = 1")',
])
@pytest.mark.parametrize('explicit', [False, True], ids=['inferred', 'declared'])
def test_t842_partial_literal_support_refuses_omitted_context(tmp_path, binding, explicit):
    path = 'tests/test_subject.py'
    before = 'def helper(): return 0\n'
    case = {'case': 'literal siblings', 'before': {path: before},
            'after': {path: before.replace('return 0', 'return 1') + binding +
                      '\ndef test_observer():\n    assert helper() == _DATA, "bug"\n'},
            'tests': [path + '::test_observer'],
            'proof': {'subjects': [path + '::helper'],
                      'observer_support': [path + '::_DATA'] if explicit else [], 'red_message': 'bug'}}
    with pytest.raises(model.ProofRefusal):
        assemble_t842_case(tmp_path, case)


@pytest.mark.parametrize('extra', [
    'import operator\nTOKEN = operator.attrgetter("modules")',
    'from operator import attrgetter as lookup\nTOKEN = lookup("modules")',
    'from operator import methodcaller as invoke\nTOKEN = invoke("__getattribute__", "modules")',
    'import operator\nTOKEN = operator.methodcaller("__getattribute__", "modules")',
    'from unittest import mock as fake\nmock = fake',
    'with mock.patch("unittest.mock.patch.object", callback): pass',
    'mock.patch.object = callback',
    'from unittest.mock import patch as replace\nwith replace("unittest.mock.patch.object", callback): pass',
    'import unittest.mock as replacement\nwith replacement.patch("unittest.mock.patch.object", callback): pass',
    'with mock.patch.object(factory(), "__enter__", callback): pass',
])
def test_t842_indirect_namespace_and_mock_siblings_are_refused(tmp_path, t842_cases, extra):
    case = t842_cases[1]
    case['after']['tests/test_pb_duplicate_arms.py'] += '\n' + extra + '\n'
    with pytest.raises(model.ProofRefusal):
        assemble_t842_case(tmp_path, case)


def t842_immutable_case(binding='_DATA = 1', expected='_DATA'):
    return {'case': 'immutable support',
            'before': {'app.py': 'def value(): return 0\n',
                       'tests/test_observer.py': 'def test_control(): pass\n'},
            'after': {'app.py': 'def value(): return 1\n',
                      'tests/test_observer.py': binding + '\ndef test_observer():\n    import app\n'
                      f'    assert app.value() == {expected}, "bug"\n'},
            'tests': ['tests/test_observer.py::test_observer'],
            'proof': {'subjects': ['app.py::value'], 'observer_support': [], 'red_message': 'bug'}}


@pytest.mark.parametrize('binding,expected', [('_DATA = 1', '_DATA'),
                                          ('_DATA = (1, ("test", b"data"))', '_DATA[0]')])
def test_t842_immutable_support_runs_the_same_observer_on_base_and_head(repository, binding, expected):
    case = t842_immutable_case(binding, expected)
    check = Check(case['case'], Kind.DEMONSTRATING, tuple(case['tests']), proof=model.Proof.parse(case['proof']))
    results, _, _ = round_result(repository, case['before'], case['after'], check)
    assert results[0].verdict == Verdict.PASS, results
    assert (results[0].tests[0].base, results[0].tests[0].head) == (Outcome.ASSERTION, Outcome.PASSED)


@pytest.mark.parametrize('mock_import,patch', [('from unittest import mock', 'mock.patch.object'),
                                             ('import unittest.mock', 'unittest.mock.patch.object')])
def test_t842_module_import_of_unittest_mock_is_authenticated(tmp_path, mock_import, patch):
    case = t842_safe_loader_case()
    path = 'tests/test_observer.py'
    case['after'][path] = case['after'][path].replace('from unittest import mock', mock_import).replace(
        'mock.patch.object', patch)
    receipt, _, head, staged = assemble_t842_case(tmp_path, case)
    assert (staged / path).read_bytes() == (head / path).read_bytes()
    assert path in receipt['support_sha256']


def test_t842_real_pb_data_can_be_repaired_with_a_new_support_factory(repository, t842_cases):
    # Planner-contracts preflight suggestion: keep all mutable initialization in
    # the copied factory rather than inferring one assignment out of its context.
    case = t842_cases[0]
    path = 'tests/test_pbtest_capability_cohorts.py'
    source = case['after'][path]
    start, end = source.index('_X86_CONFIG ='), source.index('@pytest.mark.parametrize')
    value = source[start:end].split('=', 1)[1].strip()
    case['after'][path] = source[:start] + 'def _x86_config():\n    return ' + value + '\n\n' + source[end:].replace(
        '_X86_CONFIG', '_x86_config()')
    case['proof']['observer_support'] = [path + '::_x86_config']
    check = Check('PB factory repair', Kind.DEMONSTRATING, tuple(case['tests']),
                  proof=model.Proof.parse(case['proof']))
    results, _, _ = round_result(repository, case['before'], case['after'], check)
    assert results[0].verdict == Verdict.PASS, results
    assert (results[0].tests[0].base, results[0].tests[0].head) == (Outcome.ASSERTION, Outcome.PASSED)
    assert path + '::_x86_config' in results[0].proof['support_sha256']


@pytest.mark.parametrize('shadow', ['unittest.py', 'src/unittest/mock.py', 'tests/pathlib.py', 'importlib/util.py'])
def test_t842_loader_trust_rejects_repository_stdlib_shadows(tmp_path, t842_cases, shadow):
    case = t842_cases[1]
    case['after'][shadow] = 'TOKEN = 1\n'
    case['proof']['observer_support'].append(shadow)
    with pytest.raises(model.ProofRefusal, match='unshadowed standard-library'):
        assemble_t842_case(tmp_path, case)


@pytest.mark.parametrize('binding', [
    'TOKEN = _DATA = 0\n_DATA = 1',
    'TOKEN, _DATA = (0, 0)\n_DATA = 1',
    'TOKEN = 0; _DATA = 0\n_DATA = 1',
    'TOKEN = 0; _DATA = TOKEN\n_DATA = 1',
])
def test_t842_declared_literal_cannot_hide_a_second_binding_in_its_source_line(repository, binding):
    # #842/#645 sibling: another declared name must not smuggle omitted data.
    case = json.loads((Path(__file__).parent / 'fixtures/subject_observer/review_842.json').read_text())[0]
    path = 'tests/test_observer.py'
    source = case['after'][path]
    observer = source[source.index('def test_observer():'):].replace('_DATA["expected"]', '_DATA')
    case['after'][path] = binding + '\n\n' + observer
    case['proof']['observer_support'] = [path + '::TOKEN']
    check = Check('declared literal sibling', Kind.DEMONSTRATING, tuple(case['tests']),
                  proof=model.Proof.parse(case['proof']))
    results, _, _ = round_result(repository, case['before'], case['after'], check)
    assert results[0].verdict == Verdict.FAIL, results
    assert 'Proof assembly refused' in results[0].tests[0].message, results
    assert results[0].tests[0].base == Outcome.UNCOLLECTED, results


@pytest.mark.parametrize('write', ['match 1:\n    case TOKEN: pass', 'TOKEN += 1', '(TOKEN := 1)'])
def test_t842_class_literal_support_cannot_hide_writes_in_one_compound_statement(tmp_path, write):
    from textwrap import indent

    path = 'tests/test_subject.py'
    before = 'class Context: pass\ndef helper(): return 0\n'
    after = ('class Context:\n    TOKEN = 0\n' + indent(write, '    ') +
             '\ndef helper(): return 1\ndef test_observer():\n    assert helper() == Context.TOKEN, "bug"\n')
    case = {'case': 'class literal writes', 'before': {path: before}, 'after': {path: after},
            'tests': [path + '::test_observer'],
            'proof': {'subjects': [path + '::helper'], 'observer_support': [path + '::Context::TOKEN'],
                      'red_message': 'bug'}}
    with pytest.raises(model.ProofRefusal):
        assemble_t842_case(tmp_path, case)


def test_t842_r2_review_probes(repository):
    import textwrap

    header = '''from pathlib import Path
import sys
import unittest
from unittest import mock
from string import Formatter
from types import SimpleNamespace

def load_subject():
    import importlib.util
    path = Path(__file__).resolve().parents[1] / "app.py"
    spec = importlib.util.spec_from_file_location("subject", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module
'''
    probes = {
        'patch-declared-subject': '''
            def replacement():
                return int("# head" in (Path(__file__).parents[1] / "app.py").read_text())

            def test_observer():
                subject = load_subject()
                with mock.patch.object(subject, "value", replacement), mock.patch.object(sys, "argv", []):
                    assert subject.value() == 1, "bug"
        ''',
        'alias-method-mutation': '''
            subject = load_subject()
            alias = subject.STATE
            alias.update(expected=int("# head" in (Path(__file__).parents[1] / "app.py").read_text()))

            def test_observer():
                with mock.patch.object(sys, "argv", []):
                    assert subject.value() == 1, "bug"
        ''',
        'formatter-sys-modules': '''
            modules = Formatter().get_field("0.__init__.__globals__[sys].modules", (unittest.TestCase,), {})[0]
            fixed = "# head" in (Path(__file__).parents[1] / "app.py").read_text()
            modules.update({"app": SimpleNamespace(value=lambda: int(fixed))})

            def test_observer():
                with mock.patch.object(sys, "argv", []):
                    import app
                    assert app.value() == 1, "bug"
        ''',
    }
    accepted = []
    for name, probe in probes.items():
        path = repository[0] / 'tests/test_observer.py'
        if path.exists():
            path.unlink()
        subject = 'STATE = {"expected": 1}\n\ndef value():\n    # base\n    return STATE["expected"]\n'
        before = {'app.py': subject}
        after = {'app.py': subject.replace('# base', '# head'),
                 'tests/test_observer.py': header + '\n' + textwrap.dedent(probe)}
        check = Check(name, Kind.DEMONSTRATING, ('tests/test_observer.py::test_observer',),
                      proof=model.Proof(('app.py::value',), ('tests/test_observer.py',), 'bug'))
        results, _, _ = round_result(repository, before, after, check)
        print('R2_PROBE', name, results, flush=True)
        if results[0].verdict == Verdict.PASS:
            assert (results[0].tests[0].base, results[0].tests[0].head) == (Outcome.ASSERTION, Outcome.PASSED)
            accepted.append(name)
        else:
            assert results[0].verdict == Verdict.FAIL, results
            assert results[0].tests[0].base == Outcome.UNCOLLECTED, results
            assert 'Proof assembly refused' in results[0].tests[0].message, results
    assert not accepted, 'Forged proofs accepted for unchanged subject behavior: ' + ', '.join(accepted)
@pytest.mark.parametrize('shape', ['partial-constructor', 'formatter-without-loader'])
def test_t842_r2_main_siblings(repository, shape):
    path = 'tests/test_observer.py'
    subject = 'def value():\n    # base\n    return 1\n'
    before = {'app.py': subject, path: 'def test_control(): pass\n'}
    if shape == 'partial-constructor':
        source = '_DATA = dict(expected=0)\n_DATA.update(expected=1)\n'
        source += 'def test_observer():\n    import app\n    assert app.value() == _DATA["expected"], "bug"\n'
        support = (path + '::_DATA',)
    else:
        before.pop(path)
        source = '''import unittest
from string import Formatter
from pathlib import Path
from types import SimpleNamespace
modules = Formatter().get_field("0.__init__.__globals__[sys].modules", (unittest.TestCase,), {})[0]
fixed = "# head" in (Path(__file__).parents[1] / "app.py").read_text()
modules.update({"app": SimpleNamespace(value=lambda: int(fixed))})
def test_observer():
    import app
    assert app.value() == 1, "bug"
'''
        support = (path,)
    after = {'app.py': subject.replace('# base', '# head'), path: source}
    check = Check(shape, Kind.DEMONSTRATING, (path + '::test_observer',),
                  proof=model.Proof(('app.py::value',), support, 'bug'))
    results, _, _ = round_result(repository, before, after, check)
    print('R2_MAIN_SIBLING', shape, results, flush=True)
    if results[0].verdict == Verdict.PASS:
        assert (results[0].tests[0].base, results[0].tests[0].head) == (Outcome.ASSERTION, Outcome.PASSED)
    assert results[0].verdict == Verdict.FAIL, results
    assert results[0].tests[0].base == Outcome.UNCOLLECTED
    assert 'Proof assembly refused' in results[0].tests[0].message


def t842_safe_loader_case():
    source = '''from pathlib import Path
import sys
from unittest import mock

def load_subject():
    import importlib.util
    path = Path(__file__).resolve().parents[1] / "app.py"
    spec = importlib.util.spec_from_file_location("subject", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

def test_observer():
    with mock.patch.object(sys, "argv", []):
        assert load_subject().value == 1, "bug"
'''
    return {'case': 'safe CLI loader', 'before': {'app.py': 'value = 0\n'},
            'after': {'app.py': 'value = 1\n', 'tests/test_observer.py': source},
            'tests': ['tests/test_observer.py::test_observer'],
            'proof': {'subjects': ['app.py::value'], 'observer_support': ['tests/test_observer.py'],
                      'red_message': 'bug'}}


def test_t842_r2_safe_loader_and_cli_setup_still_demonstrate(repository):
    case = t842_safe_loader_case()
    check = Check(case['case'], Kind.DEMONSTRATING, tuple(case['tests']), proof=model.Proof.parse(case['proof']))
    results, _, _ = round_result(repository, case['before'], case['after'], check)
    assert results[0].verdict == Verdict.PASS, results
    assert (results[0].tests[0].base, results[0].tests[0].head) == (Outcome.ASSERTION, Outcome.PASSED)


@pytest.mark.parametrize('extra', [
    'with mock.patch.object(load_subject(), "value", 1): pass',
    'subject = load_subject()\nwith mock.patch.object(subject, "dependency", 1): pass',
    'import app\nwith mock.patch.object(app, "value", 1): pass',
    'subject = load_subject()\nalias = subject.STATE\nalias.update(expected=0)',
    'subject = load_subject()\nmutate = subject.STATE.update\nmutate(expected=0)',
    'subject = load_subject()\ndef setup():\n    alias = subject.STATE\n    alias.clear()\nsetup()',
    'from string import Formatter as F\nTOKEN = F().get_field("0.__dict__", (), {})',
    'TOKEN = "{0.__globals__}".format(load_subject)',
    'formatter = "{x}".format_map\nTOKEN = formatter({"x": 0})',
    'subject = load_subject()\nTOKEN = "%(expected)s" % subject.STATE',
    'TOKEN = "__%(x)s" % {"x": 0}',
    'import unittest\nTOKEN = unittest.TestCase',
    'import unittest\nTOKEN = unittest.main',
    'from somewhere import mutate as len\nTOKEN = len(())',
    'def helper(helper):\n    helper()\nhelper(load_subject)',
    'import app\nTOKEN = sorted([{}], key=app.STATE.update)',
    'import app\ncallback = app.STATE.update\nTOKEN = sorted([{}], key=callback)',
    'import app\n@ app.replace_subject\ndef setup(): pass',
    'import app\nclass Support(app.ReplacingBase): pass',
    'import app\nwith app.replacing_context: pass',
])
def test_t842_r2_call_and_reflection_siblings_refuse(tmp_path, extra):
    case = t842_safe_loader_case()
    case['after']['tests/test_observer.py'] += '\n' + extra + '\n'
    with pytest.raises(model.ProofRefusal):
        assemble_t842_case(tmp_path, case)


@pytest.mark.parametrize('initializer', ['dict(expected=0)', 'list((0,))', 'set((0,))',
                                        'tuple((0,))', 'frozenset((0,))', 'int(0)', 'factory()'])
def test_t842_r2_partial_call_initializers_refuse(tmp_path, initializer):
    case = t842_immutable_case('_DATA = ' + initializer)
    case['proof']['observer_support'] = ['tests/test_observer.py::_DATA']
    with pytest.raises(model.ProofRefusal):
        assemble_t842_case(tmp_path, case)


def test_t842_r2_pure_support_functions_and_builtins_are_supported(repository):
    case = t842_immutable_case('def data():\n    return tuple(sorted(range(2)))[1]', 'data()')
    case['proof']['observer_support'] = ['tests/test_observer.py::data']
    check = Check(case['case'], Kind.DEMONSTRATING, tuple(case['tests']), proof=model.Proof.parse(case['proof']))
    results, _, _ = round_result(repository, case['before'], case['after'], check)
    assert results[0].verdict == Verdict.PASS, results
    assert (results[0].tests[0].base, results[0].tests[0].head) == (Outcome.ASSERTION, Outcome.PASSED)


@pytest.mark.parametrize('shadow', ['len = lambda value: 1', 'from app import mutate as len'])
def test_t842_r2_partial_support_cannot_trust_a_shadowed_builtin(tmp_path, shadow):
    case = t842_immutable_case('def data():\n    return len(())', 'data()')
    case['after']['tests/test_observer.py'] = shadow + '\n' + case['after']['tests/test_observer.py']
    case['proof']['observer_support'] = ['tests/test_observer.py::data']
    with pytest.raises(model.ProofRefusal):
        assemble_t842_case(tmp_path, case)


@pytest.mark.parametrize('rebinding', ['data = lambda: 1', 'match 1:\n    case data: pass',
                                     'def replace():\n    global data\n    data = lambda: 1'])
def test_t842_r2_partial_function_cannot_omit_rebinding(tmp_path, rebinding):
    case = t842_immutable_case('def data():\n    return 0\n' + rebinding, 'data()')
    case['proof']['observer_support'] = ['tests/test_observer.py::data']
    with pytest.raises(model.ProofRefusal):
        assemble_t842_case(tmp_path, case)


@pytest.mark.parametrize('mutation', [
    'data.__defaults__ = (1,)',
    'alias = data\nalias.__defaults__ = (1,)',
])
def test_t842_r2_partial_factory_cannot_omit_function_object_mutation(repository, mutation):
    path = 'tests/test_observer.py'
    subject = 'def value():\n    # base\n    return 1\n'
    source = 'def data(expected=0):\n    return expected\n' + mutation + '\n'
    source += 'def test_observer():\n    import app\n    assert app.value() == data(), "bug"\n'
    check = Check('factory object mutation', Kind.DEMONSTRATING, (path + '::test_observer',),
                  proof=model.Proof(('app.py::value',), (path + '::data',), 'bug'))
    results, _, _ = round_result(repository, {'app.py': subject, path: 'def test_control(): pass\n'},
                                {'app.py': subject.replace('# base', '# head'), path: source}, check)
    print('R2_FACTORY_OBJECT', mutation, results, flush=True)
    if results[0].verdict == Verdict.PASS:
        assert (results[0].tests[0].base, results[0].tests[0].head) == (Outcome.ASSERTION, Outcome.PASSED)
    assert results[0].verdict == Verdict.FAIL, results
    assert results[0].tests[0].base == Outcome.UNCOLLECTED
    assert 'Proof assembly refused' in results[0].tests[0].message


# #867 (found by: live probe, tessera #1455): a refusal must tell the maker what
# is allowed and how to repair, or the one repair turn repeats the same code.
T867_CASES = [
    ('hasattr', 'def helper(value):\n    return hasattr(value, "decoded")\n', ['hasattr', 'allowed builtins']),
    ('isinstance', 'def helper(value):\n    return isinstance(value, int)\n', ['isinstance', 'allowed builtins']),
    ('non-literal pure builtin', 'def helper(value):\n    return sorted(value)\n', ['sorted', 'literal arguments']),
    ('method call', 'def helper(value):\n    return value.decode()\n', ['value.decode', 'allowed builtins']),
    ('decorator', 'import pytest\n\n@pytest.fixture\ndef data():\n    return 1\n', ['pytest.fixture', 'decorator']),
    ('class bases', 'class Box(dict):\n    pass\n', ['Box', 'base class']),
    ('context manager', 'def helper():\n    with open("x") as handle:\n        return handle\n', ['with', 'sys.argv']),
    ('percent template', 'def helper(value):\n    return "%s" % value\n', ['%', 'f-string']),
    ('reflective attribute', 'TEMPLATE = "{0}"\ndef helper():\n    return TEMPLATE.format(1)\n', ['format', 'f-string']),
    ('member store', 'def helper(box):\n    box["k"] = 1\n', ['box', 'new name']),
    ('namespace api', 'def helper(value):\n    return getattr(value, "x")\n', ['getattr', 'observer test body']),
]


@pytest.mark.parametrize('name,source,expected', T867_CASES, ids=[case[0] for case in T867_CASES])
def test_t867_support_refusals_say_what_is_allowed_and_how_to_repair(name, source, expected):
    from tl3_harness import proof
    with pytest.raises(model.ProofRefusal) as refused:
        proof.validate_support(source, 'tests/test_cached_producer.py')
    message = str(refused.value)
    for text in expected:
        assert text in message, (name, message)
    assert 'observer test body' in message, (name, message)


def test_t867_call_refusal_lists_the_allowlist_generated_from_the_rule():
    """The listed builtins come from the same constant the rule uses, so they cannot drift."""
    from tl3_harness import proof
    allowed = getattr(proof, 'PURE_SUPPORT_BUILTINS', None)
    assert allowed, 'proof.PURE_SUPPORT_BUILTINS must be the allowlist the call rule uses'
    with pytest.raises(model.ProofRefusal) as refused:
        proof.validate_support('def helper(value):\n    return hasattr(value, "decoded")\n',
                               'tests/test_cached_producer.py')
    message = str(refused.value)
    assert message.startswith('Proof assembly refused tests/test_cached_producer.py::hasattr: '), message
    for builtin in sorted(allowed):
        assert builtin in message, (builtin, message)
    assert 'hasattr' not in allowed
