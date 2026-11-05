"""Regenerate the planner oracle in-process. Run on BuildBuddy, never locally.

Usage: uv run --project workers --locked python workers/tests/planner_vectors_record.py
  --output PATH [--check]
"""
import argparse
from contextlib import ExitStack, contextmanager
from copy import deepcopy
from dataclasses import replace
import gzip
import json
import os
import re
from pathlib import Path, PurePosixPath
import sys
import tempfile
from unittest.mock import patch

import pytest

from planner_vectors import (
    CORPUS, PURE, SOURCES, RehearsalFacts, Tape, exception,
    facts, load, modules, packed, replay, strings, rule_coverage, assert_same,
)
from tl3_workers.roles import planner, ROLES
from tl3_workers.forecast import Forecaster

ORIGINAL_MEASURE = Forecaster.measure


def in_process_worker(*args, **kwargs):
    """Honor child-only model setup while excluding parent-only forecast fakes."""
    import worker_process
    from langchain_core.language_models import fake_chat_models
    from langchain_core.messages import AIMessage
    model = fake_chat_models.FakeMessagesListChatModel
    launch = worker_process.LAUNCH
    custom = re.search(r'\nmodel = (\w+)\(', launch)
    if custom and custom[1] != 'FakeMessagesListChatModel':
        start = launch.index('class ' + custom[1])
        namespace = {'FakeMessagesListChatModel': model, 'AIMessage': AIMessage, 'json': json}
        exec(launch[start:custom.start()], namespace)
        model = namespace[custom[1]]
    with patch.object(fake_chat_models, 'FakeMessagesListChatModel', model), \
         patch.object(Forecaster, 'measure', ORIGINAL_MEASURE):
        return worker_process.start_worker_in_process(*args, **kwargs)


class Recorder:
    def __init__(self):
        self.cases, self.scenarios = [], {}
        self.active = False
        self.source = self.node = ''
        self.ordinal = 0
        self.temporary_root = None

    def case(self, operation):
        self.ordinal += 1
        return {'id': f'{self.node}#{self.ordinal}', 'source': self.source, 'operation': operation}

    def observe(self, case, invoke, after):
        self.active = True
        try:
            result = invoke()
        except Exception as error:
            case['outcome'] = {'error': exception(error)}
            raise
        else:
            case['outcome'] = {'return': packed(result)}
            return result
        finally:
            self.active = False
            after(case['outcome'])
            self.cases.append(case)

    def check(self, original):
        def recorded(inputs, reply, *, rehearsals=None):
            if self.active:
                return original(inputs, reply, rehearsals=rehearsals)
            case = self.case('check_reply')
            case.update(inputs=deepcopy(inputs), reply=deepcopy(reply), has_rehearsals=rehearsals is not None)
            tape = Tape()
            def invoke():
                with facts(tape):
                    ledger = RehearsalFacts(tape, rehearsals) if rehearsals is not None else None
                    return original(inputs, reply, rehearsals=ledger)
            def after(result):
                result['reply'] = deepcopy(reply)
                case['facts'] = tape.rows
            return self.observe(case, invoke, after)
        return recorded

    def correction(self, operation, original):
        def recorded(mismatch, reply):
            if self.active:
                return original(mismatch, reply)
            case = self.case(operation)
            case.update(reply=deepcopy(reply), mismatch=exception(mismatch))
            tape = Tape()
            def invoke():
                with facts(tape):
                    return original(mismatch, reply)
            def after(result):
                result['reply'] = deepcopy(reply)
                case['facts'] = tape.rows
            return self.observe(case, invoke, after)
        return recorded

    def pure(self, operation, original):
        def recorded(*args, **kwargs):
            if self.active:
                return original(*args, **kwargs)
            # proof_applicability's base callback is itself an ordered fact.
            callback = kwargs.pop('defined', None)
            if operation == 'evidence.proof_applicability' and len(args) > 2:
                args, callback = args[:2], args[2]
            case = self.case(operation)
            case.update(args=packed(args), kwargs=packed(kwargs), defined=callback is not None)
            tape = Tape()
            actual = dict(kwargs)
            if callback is not None:
                actual['defined'] = tape.wrap('defined', callback)
            def invoke():
                with facts(tape):
                    result = original(*args, **actual)
                    return list(result) if operation == 'planner.mentions' else result
            def after(result):
                result.update(args=packed(args), kwargs=packed(kwargs))
                case['facts'] = tape.rows
            return self.observe(case, invoke, after)
        return recorded

    @pytest.hookimpl(hookwrapper=True, trylast=True)
    def pytest_runtest_call(self, item):
        self.temporary_root = str(item.config._tmp_path_factory.getbasetemp())
        self.node = item.nodeid
        self.source = item.path.stem.removeprefix('test_')
        self.ordinal = 0
        replacements = []
        check = self.check(planner.check_reply)
        replacements.append((planner.check_reply, check))
        for module_name, module in modules().items():
            for name in PURE[module_name]:
                original = getattr(module, name)
                replacements.append((original, self.pure(module_name + '.' + name, original)))
        # Tests import helpers both by module and by name, often inside the test.
        with ExitStack() as stack:
            by_id = {id(original): replacement for original, replacement in replacements}
            for module in list(sys.modules.values()):
                if module is None:
                    continue
                filename = getattr(module, '__file__', '') or ''
                if not ('tl3_workers' in filename or '/workers/tests/' in filename):
                    continue
                for name, value in list(vars(module).items()):
                    replacement = by_id.get(id(value))
                    if replacement is not None:
                        stack.enter_context(patch.object(module, name, replacement))
            stack.enter_context(patch.dict(ROLES, planner=replace(ROLES['planner'], check_reply=check)))
            for name in ('constrain', 'constrain_local'):
                stack.enter_context(patch.object(planner.PlanConsistencyError, name,
                    self.correction(name, getattr(planner.PlanConsistencyError, name))))
            import worker_process
            import planner_fixtures
            worker_start = worker_process.start_worker
            for module in list(sys.modules.values()):
                if module is None or '/workers/tests/' not in (getattr(module, '__file__', '') or ''):
                    continue
                for name, value in list(vars(module).items()):
                    if value is worker_start:
                        stack.enter_context(patch.object(module, name, in_process_worker))
            # The helper's global was captured by import before the patch above.
            stack.enter_context(patch.object(planner_fixtures, 'start_worker', in_process_worker))
            yield

    def pytest_runtest_logreport(self, report):
        if report.when == 'call':
            self.scenarios[report.nodeid] = report.outcome


def heuristics():
    descriptions = [
        'Edit test_api.', 'Edit one two three four five six test_api.',
        'Edit one two three four five six seven test_api.',
        'Do not edit test_api.', 'Never edit test_api.', 'Don’t edit test_api.',
        'Keep test_api unchanged.', 'Edit test_api but leave the other unchanged.',
        'Edit test_api unchanged.', 'Edit test_api one two unchanged.',
        'Edit test_api one two three unchanged.', 'Edit something and test_api.',
        'Edit something because test_api.', 'Add assertions to test_api.',
        'Add assertions into test_api.', 'Add test_api.',
        'Edit tests/test_api.py.', 'Edit test_api.py.',
        'Edit something. test_api stays.', 'Edit something; test_api stays.',
        'Edit something\ntest_api stays.', 'Edit test_api; preserve test_api.',
    ]
    descriptions += [f'Edit something {clause} test_api.' for clause in sorted(planner.CLAUSES)]
    descriptions += [f'{hold} edit test_api.' for hold in sorted(planner.HOLDS)]
    cases = []
    for index, description in enumerate(descriptions):
        mentions = list(planner.mentions(description, 'test_api'))
        cases.append({'id': f'english-{index:02}', 'description': description, 'test': 'test_api',
                      'mentions': packed(mentions),
                      'verdict': any(planner.asks_to_edit(*mention) for mention in mentions)})
    return cases



def globs():
    pairs = [
        ('a/b.py', '*.py'), ('a.py', '**/*.py'), ('a/b.py', '**/*.py'),
        ('a/b/c.py', 'a/*.py'), ('a/b/c.py', 'a/**/c.py'), ('a/c.py', 'a/**/c.py'),
        ('a', 'a/**'), ('a/b', 'a/**'), ('.hidden.py', '*.py'),
        ('a//b.py', 'a/b.py'), ('./a.py', 'a.py'), ('a/./b.py', 'a/b.py'),
        ('a.py', '{a,b}.py'), ('{a,b}.py', '{a,b}.py'),
        ('a.py', r'\a.py'), ('a.py', '[ab].py'), ('c.py', '[!ab].py'),
        ('^.py', '[^ab].py'), ('c.py', '[^ab].py'), ('é.py', '?.py'),
        ('x[.py', 'x[.py'), ('a.py', 'A.py'), ('/a.py', '*.py'),
        ('a/b.py', 'a/**b.py'), ('a.py', ''), ('', '**'),
    ]
    return [{'id': f'glob-{index:02}', 'path': path, 'pattern': pattern,
             'verdict': PurePosixPath(path).full_match(pattern)}
            for index, (path, pattern) in enumerate(pairs)]

def git_observations(blob, *, size=None, size_error=None, blob_error=None):
    """Supply external Git observations, never the base-definition decision."""
    def observed(repo, *args):
        if args[:2] == ('cat-file', '-s'):
            if size_error is not None:
                raise size_error
            return str(len(blob) if size is None else size).encode()
        assert args[:2] == ('cat-file', 'blob'), args
        if blob_error is not None:
            raise blob_error
        return blob
    return observed


def observation_cases(recorder, inputs, forecast, execution):
    from subprocess import CalledProcessError
    from types import SimpleNamespace
    from tl3_workers.tools import Worktree

    recorder.source, recorder.node, recorder.ordinal = 'planner_vectors_extra', 'extra/base-observations', 0
    base = recorder.pure('planner.base_definition', planner.base_definition)
    node = 'tests/test_base.py::test_present'
    source = b'def test_present():\n    assert True\n'
    for git in (
        git_observations(source),
        git_observations(b'def test_absent(): pass\n'),
        git_observations(b'invalid Python source!'),
        git_observations(b'\xff'),
        git_observations(source + b'#' * (planner.HARD_CAP_BYTES - len(source))),
        git_observations(source, size=planner.HARD_CAP_BYTES + 1),
        git_observations(source, size='not-an-integer'),
        git_observations(source, size_error=OSError('size unavailable')),
        git_observations(source, blob_error=CalledProcessError(128, ['git', 'cat-file', 'blob', 'immutable-base:tests/test_base.py'])),
        git_observations(('é' * (planner.HARD_CAP_BYTES // 2 + 1)).encode(), size=planner.HARD_CAP_BYTES),
    ):
        with patch.object(planner, 'git', git):
            base(node, 'immutable-base')

    previous = Path.cwd()
    with tempfile.TemporaryDirectory(dir=recorder.temporary_root) as temporary:
        os.chdir(temporary)
        try:
            Path('tests').mkdir()
            path = Path('tests/test_base.py')
            for content in (source, source + b'#' * (planner.HARD_CAP_BYTES - len(source)), b'\xff',
                            ('é' * (planner.HARD_CAP_BYTES // 2 + 1)).encode()):
                path.write_bytes(content)
                base(node)
            edits = recorder.pure('planner.edit_violations', planner.edit_violations)
            edits([], {'files': ['tests/test_base.py'], 'edits_tests': [node],
                       'checks': [{'kind': 'demonstrating', 'node_ids': [node]}]})
            path.unlink()
            base(node)
            with patch.object(planner, 'Worktree', lambda root: Worktree('absent-root')):
                base(node)
            path.write_bytes(source)
            close = Worktree.__exit__
            def failing_close(tree, *exc):
                close(tree, *exc)
                raise OSError('worktree close unavailable')
            with patch.object(Worktree, '__exit__', failing_close):
                base(node)
            file = Worktree._file
            @contextmanager
            def failing_file_close(tree, *args, **kwargs):
                with file(tree, *args, **kwargs) as stream:
                    yield stream
                raise OSError('file close unavailable')
            with patch.object(Worktree, '_file', failing_file_close):
                base(node)

            # The immutable base lacks this test, but local correction reads HEAD.
            recorder.node, recorder.ordinal = 'extra/local-base-sha-asymmetry', 0
            path.write_text('def test_only_at_head(): pass\n')
            head_node = 'tests/test_base.py::test_only_at_head'
            with patch.object(planner, 'git', git_observations(b'def test_only_at_base(): pass\n')):
                base(head_node, 'immutable-base')
            from planner_fixtures import LEAF
            draft = deepcopy(LEAF)
            draft['checks'] = [dict(kind='regression', node_ids=['tests/test_base.py'], goal_ids=[0])]
            request = deepcopy(inputs)
            request.update(base_sha='immutable-base', repair_plan_consistency=True)
            check = recorder.check(planner.check_reply)
            with patch.object(planner, 'Forecaster', lambda _: SimpleNamespace(measure=lambda *args: deepcopy(forecast), problems=[])), \
                 patch.object(planner, 'execution_forecast', lambda *args, **kwargs: deepcopy(execution)):
                try:
                    check(request, deepcopy(draft))
                except planner.PlanConsistencyError as error:
                    mismatch = error
                else:
                    raise AssertionError('the non-exact base selector must request local correction')
            reply = deepcopy(draft)
            reply['checks'][0]['node_ids'] = [head_node]
            local = recorder.correction('constrain_local', planner.PlanConsistencyError.constrain_local)
            local(mismatch, reply)
        finally:
            os.chdir(previous)


def diagnostic_and_byte_cases(recorder, inputs, forecast, execution):
    from types import SimpleNamespace
    from planner_fixtures import LEAF
    from tl3_workers import generated_scope, evidence

    recorder.source, recorder.node, recorder.ordinal = 'planner_vectors_extra', 'extra/diagnostics-and-byte-traps', 0
    prohibition = recorder.pure('generated_scope.check_ingest_generation_scope', generated_scope.check_ingest_generation_scope)
    try:
        prohibition({'issue': {'title': '', 'body': ''}, 'tool_capabilities': [
            {'tool': 'run_generator', 'executor': 'prismabuild_generator'}]},
            {'kind': 'brief', 'brief': 'Keep generated outputs unchanged.', 'goals': []})
    except generated_scope.GeneratedOutputProhibition:
        pass

    check = recorder.check(planner.check_reply)
    applicable = recorder.pure('evidence.proof_applicability', evidence.proof_applicability)
    with patch.object(planner, 'Forecaster', lambda _: SimpleNamespace(measure=lambda *args: deepcopy(forecast), problems=[])), \
         patch.object(planner, 'execution_forecast', lambda *args, **kwargs: deepcopy(execution)):
        numeric_inputs = deepcopy(inputs)
        numeric_inputs['ingest']['goals'] *= 2
        for goal_ids in ([0, 1], [0, 1.0], [0, True]):
            reply = deepcopy(LEAF)
            reply['checks'] = [dict(deepcopy(reply['checks'][0]), goal_ids=goal_ids)]
            applicable(numeric_inputs['ingest']['goals'], reply['checks'])
            check(deepcopy(numeric_inputs), reply)

        unicode_inputs = deepcopy(inputs)
        unicode_inputs['forecast'].update(generator='configured generator', generated_files=[])
        unicode_inputs['protected_paths'] = ['*.py']
        reply = deepcopy(LEAF)
        reply['checks'] = reply['checks'][:1]
        reply['files'] = ['𐀀.py', 'Ω.py', 'é.py', '\ue000.py', 'Z.py']
        with patch.object(planner, 'configured_outputs', lambda *args: []):
            check(unicode_inputs, reply)

        for multibyte in (False, True):
            for target in (2047, 2048, 2049):
                reply = deepcopy(LEAF)
                reply['checks'] = [dict(kind='regression', node_ids=['tests/.py'], goal_ids=[0])]
                selectors = {0: reply['checks'][0]['node_ids']}
                minimum = len(str(planner.PlanConsistencyError([], draft=reply, selectors=selectors)).encode())
                prefix = 'é' * 16 if multibyte else ''
                node = 'tests/' + prefix + 'x' * (target - minimum - len(prefix.encode())) + '.py'
                reply['checks'][0]['node_ids'] = [node]
                selectors = {0: [node]}
                mismatch = planner.PlanConsistencyError([], draft=reply, selectors=selectors)
                message = str(mismatch)
                assert len(message.encode()) == target
                boundary_inputs = deepcopy(inputs)
                boundary_inputs['repair_plan_consistency'] = True
                try:
                    check(boundary_inputs, reply)
                except planner.PlanConsistencyError:
                    pass
                recorder.cases[-1]['repair_boundary'] = {
                    'bytes': target, 'characters': len(message), 'selectors': packed(selectors)}


def pb2710_cases(recorder, inputs, forecast, execution):
    """Replay the trace projection with recorded budget observations, as other extras do."""
    from types import SimpleNamespace
    from planner_fixtures import LEAF

    fixture = json.loads((Path(__file__).parent / 'fixtures/planner-pb2710.json').read_text())
    request = deepcopy(inputs)
    request.pop('base_sha', None)
    request['repair_plan_consistency'] = True
    request['ingest']['goals'] = fixture['goals']
    draft = {**deepcopy(LEAF), 'files': fixture['files'], 'checks': fixture['checks']}
    check = recorder.check(planner.check_reply)
    constrain = recorder.correction('constrain', planner.PlanConsistencyError.constrain)
    previous = Path.cwd()
    with tempfile.TemporaryDirectory(dir=recorder.temporary_root) as temporary:
        os.chdir(temporary)
        try:
            for file in fixture['files']:
                Path(file).parent.mkdir(parents=True, exist_ok=True)
                Path(file).write_text('')
            Path('tests/test_checkout_recovery_1465.py').write_text(
                'def _git(): pass\n'
                'def test_missing_or_conflicting_action_snapshot_and_ending_proof_keeps_original():\n'
                '    assert True\n')
            with patch.object(planner, 'Forecaster', lambda _: SimpleNamespace(measure=lambda *args: deepcopy(forecast), problems=[])), \
                 patch.object(planner, 'execution_forecast', lambda *args, **kwargs: deepcopy(execution)):
                for resolution in ('accepted', 'unrelated_change'):
                    recorder.source, recorder.node, recorder.ordinal = 'planner_vectors_extra', 'extra/pb2710/' + resolution, 0
                    terminal_inputs = {**deepcopy(request), 'repair_plan_consistency': False}
                    terminal = deepcopy(draft)
                    check(terminal_inputs, terminal)
                    assert terminal['violations'] == [fixture['violation'], 'Planner coverage correction attempts: 0.']
                    try:
                        check(deepcopy(request), deepcopy(draft))
                    except planner.PlanConsistencyError as error:
                        mismatch = error
                    else:
                        raise AssertionError('PB #2710 must be eligible for coverage repair')
                    assert mismatch.missing_goals == {3: fixture['goals'][3]['description']}
                    assert mismatch.repair_text == fixture['violation'] + '\n' + fixture['repair_instruction']
                    corrected = deepcopy(draft)
                    corrected['checks'][0]['goal_ids'].append(3)
                    if resolution == 'unrelated_change':
                        corrected['estimated_exploration_turns'] += 1
                    check(deepcopy(request), corrected)
                    assert corrected['violations'] == []
                    constrain(mismatch, corrected)
                    if resolution == 'accepted':
                        assert corrected['violations'] == []
                    else:
                        assert 'Correction changes declarations outside the test consistency error.' in corrected['violations']
        finally:
            os.chdir(previous)


def extra_cases(recorder):
    """Reach every correction rejection without expanding the production rules."""
    from planner_fixtures import LEAF, SPLIT
    recorder.source, recorder.node, recorder.ordinal = 'planner_vectors_extra', 'extra/corrections', 0
    original = planner.PlanConsistencyError.constrain
    constrain = recorder.correction('constrain', original)
    local = recorder.correction('constrain_local', planner.PlanConsistencyError.constrain_local)
    draft = deepcopy(LEAF)
    draft['violations'] = []
    node = draft['checks'][0]['node_ids'][0]
    draft['edits_tests'] = [node]
    mismatch = planner.PlanConsistencyError([node], ['tests/test_old.py::test_old'], draft=draft)
    mutations = [
        lambda r: r.update(kind='split'),
        lambda r: r['files'].append('extra.py'),
        lambda r: r['files'].pop(),
        lambda r: r.update(no_design_questions=False),
        lambda r: r['edits_tests'].append('tests/test_other.py::test_other'),
        lambda r: r['checks'].pop(),
        lambda r: r['checks'][0].update(goal_ids=[999]),
        lambda r: r['checks'].append({'kind': 'regression', 'node_ids': ['tests/x.py::test_x'], 'goal_ids': [0]}),
        lambda r: r['checks'].append({'kind': 'demonstrating', 'node_ids': [node], 'goal_ids': [0],
                                      'forcing': {'symptom': 'New forcing', 'environment': {'CASE': 'new'}, 'patches': []}}),
        lambda r: r['checks'].append({'kind': 'demonstrating', 'node_ids': [node], 'goal_ids': [0],
                                      'declared_symptom': {'exception': 'ValueError'}}),
        lambda r: r['checks'][0].update(kind='regression'),
        lambda r: r['checks'][0].update(node_ids=[]),
        lambda r: r['checks'][1]['node_ids'].append('tests/x.py::test_x'),
        lambda r: r.update(kind='cant_meet', files=['extra.py']),
        lambda r: None,
    ]
    for change in mutations:
        reply = deepcopy(draft)
        change(reply)
        constrain(mismatch, reply)
    selector = planner.PlanConsistencyError([], draft=draft, selectors={0: ['tests/test_greeting.py']})
    proof_draft = deepcopy(draft)
    del proof_draft['checks'][0]['proof']['red_message']
    missing_proof = planner.PlanConsistencyError([], draft=proof_draft, missing_proof={0: {'red_message'}})
    subject_draft = deepcopy(draft)
    subject_draft['checks'][0]['proof']['subjects'] = ['greeting.py::previous']
    subjects = planner.PlanConsistencyError([], draft=subject_draft, missing_subjects={0: {'greeting.py::target'}})
    for mismatch, change in [
        (selector, lambda r: r.update(edits_tests=[])),
        (selector, lambda r: r['checks'].pop()),
        (selector, lambda r: r['checks'][0].update(goal_ids=[1])),
        (selector, lambda r: r['checks'][0].update(node_ids=['tests/test_greeting.py'])),
        (selector, lambda r: r['checks'][0].update(node_ids=['tests/test_missing.py::test_missing'])),
        (missing_proof, lambda r: r['checks'][0]['proof'].update(observer_support=['other.py'], red_message='filled')),
        (subjects, lambda r: r['checks'][0]['proof'].update(subjects=['other.py::unrelated'])),
        (subjects, lambda r: r['checks'][0]['proof'].update(subjects=['greeting.py::previous', 'greeting.py'])),
    ]:
        reply = deepcopy(mismatch.draft)
        change(reply)
        constrain(mismatch, reply)
        local(mismatch, deepcopy(reply))
    # Boundary-only shape cases cannot be obtained through model schemas.
    from types import SimpleNamespace
    import checker_stub
    responses = [row['response']['result'] for row in checker_stub.vectors()
                 if row['document'].get('kind') in ('forecast', 'execution')]
    forecast = next(value for value in responses if value.get('tokenizer') == 'o200k_base' and value.get('fits'))
    execution = next(value for value in responses if value.get('exploration_declared') and value.get('fits'))
    config = {'fit_tokens': 100000, 'context_tokens': 150000, 'overhead_tokens': {'prompt': 1}}
    inputs = {'must_split': False, 'ingest': {'brief': 'fixture', 'goals': [{'description': 'Preserve behavior.', 'verification': 'test'}]},
              'protected_paths': [], 'forecast': config, 'base_sha': 'vector-base'}
    check = recorder.check(planner.check_reply)
    plans = []
    for change in [lambda r: r.update(files=[]), lambda r: r.update(no_design_questions=False),
                   lambda r: r.update(lands_on_its_own=False),
                   lambda r: r['checks'][0].update(node_ids=[]), lambda r: r['checks'][0].update(goal_ids=[]),
                   lambda r: r.update(execution={'fits': True})]:
        reply = deepcopy(LEAF)
        reply['checks'] = reply['checks'][:1]
        change(reply)
        plans.append(reply)
    for reason in ('multiple_repos', 'out_of_room', 'does_not_fit', 'execution_budget'):
        reply = deepcopy(SPLIT)
        reply['reason'] = reason
        plans.append(reply)
    piece_without_goal = deepcopy(SPLIT)
    piece_without_goal['pieces'][0]['goal_ids'] = []
    plans.append(piece_without_goal)
    oversized = deepcopy(LEAF)
    oversized['edits_tests'] = [node]
    plans.append(oversized)
    long_selectors = deepcopy(LEAF)
    long_selectors['checks'] = [dict(deepcopy(LEAF['checks'][0]), node_ids=[f'tests/test_missing_{index}_' + 'x' * 200 + '.py'])
                                for index in range(20)]
    plans.append(long_selectors)
    for reply in plans:
        source = b'def test_api(): pass\ndef test_existing(): pass\n'
        size = planner.HARD_CAP_BYTES + 1 if reply is oversized else len(source)
        with patch.object(planner, 'git', git_observations(source, size=size)), \
             patch.object(planner, 'Forecaster', lambda _: SimpleNamespace(measure=lambda *args: deepcopy(forecast), problems=[])), \
             patch.object(planner, 'execution_forecast', lambda *args, **kwargs: deepcopy(execution)):
            check(deepcopy(inputs), reply)
    nofit = next(value for value in responses if value.get('tokenizer') == 'o200k_base' and not value.get('fits'))
    reply = deepcopy(SPLIT)
    reply['reason'] = 'execution_budget'
    with patch.object(planner, 'Forecaster', lambda _: SimpleNamespace(measure=lambda *args: deepcopy(nofit), problems=[])), \
         patch.object(planner, 'execution_forecast', lambda *args, **kwargs: deepcopy(execution)):
        check(deepcopy(inputs), reply)
    observation_cases(recorder, inputs, forecast, execution)
    diagnostic_and_byte_cases(recorder, inputs, forecast, execution)
    pb2710_cases(recorder, inputs, forecast, execution)


def generate(selection=None):
    recorder = Recorder()
    root = Path(__file__).resolve().parents[2]
    os.chdir(root / 'workers')
    paths = [f'tests/test_{source}.py' for source in SOURCES if source not in ('workers', 'recall')]
    paths += ['tests/test_workers.py', 'tests/test_branch_publication.py']
    # Only the generated-scope and memory-citation scenarios belong to this slice.
    class Selection:
        def pytest_collection_modifyitems(self, items):
            items[:] = [item for item in items if item.path.stem != 'test_workers' or
                        any(word in item.name for word in ('generated', 'freeze', 'freezes', 'memories'))]
    # Fixture commits are facts too; fix their timestamps before setup creates them.
    dates = dict.fromkeys(('GIT_AUTHOR_DATE', 'GIT_COMMITTER_DATE'), '2000-01-01T00:00:00+00:00')
    with patch.dict(os.environ, dates):
        status = pytest.main(['-q', *paths, *(['-k', selection] if selection else [])], plugins=[recorder, Selection()])
    extra_cases(recorder)
    recorder.source, recorder.node, recorder.ordinal = 'recall', 'extra/recall', 0
    citation = recorder.pure('recall.check_citations', modules()['recall'].check_citations)
    citation({'memories': []}, {'memory_ids': []})
    for reply in ({'memory_ids': ['unknown']}, {'memory_flags': [{'id': 'unknown'}]}):
        try:
            citation({'memories': []}, reply)
        except ValueError:
            pass
    document = {'format': 'tl3.planner-vectors.v1', 'cases': recorder.cases,
                'scenarios': recorder.scenarios, 'english': heuristics(), 'globs': globs()}
    # Persist diagnostics even on a failing source scenario, but never claim drift
    # success or publish it as the committed oracle until every scenario passes.
    document['source_exit'] = int(status)
    text = json.dumps(document, ensure_ascii=False)
    if recorder.temporary_root:
        text = text.replace(recorder.temporary_root, '/__planner_pytest__')
    text = text.replace(str(root), '/__planner_repo__')
    return json.loads(text)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=CORPUS)
    parser.add_argument('--select')
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    expected = load() if args.check else None
    document = generate(args.select)
    data = gzip.compress(json.dumps(document, ensure_ascii=False, separators=(',', ':')).encode(), mtime=0)
    if not args.check:
        args.output.write_bytes(data)
    coverage = rule_coverage(document)
    if not args.check:
        args.output.with_suffix('.coverage.json').write_text(json.dumps(strings(document), ensure_ascii=False, indent=2))
        args.output.with_suffix('.rules.json').write_text(json.dumps(coverage, ensure_ascii=False, indent=2))
    print('Missing diagnostic templates:', [rule for rule, ids in coverage.items() if not ids])
    print(f"recorded {len(document['cases'])} calls from {len(document['scenarios'])} scenarios")
    if document['source_exit']:
        return document['source_exit']
    for case in document['cases']:
        replay(case)
    if not args.select:
        assert all(coverage.values()), [rule for rule, ids in coverage.items() if not ids]
    if args.check:
        assert_same(document, expected, 'planner corpus drift; regenerate on BuildBuddy')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
