"""The temporary Kani gate separates PR proofs from the full nightly run."""
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import yaml

import kani_gate

ROOT = Path(__file__).resolve().parents[2]
HEAD = 'c' * 40
SUCCESS = '''Checking harness proofs::apply_stop...
 ** 2 of 2 cover properties satisfied
VERIFICATION:- SUCCESSFUL
Verification Time: 12.5s
Complete - 1 successfully verified harnesses, 0 failures, 1 total.
'''
FAILURE = '''Checking harness proofs::progress_from_unplanned...
Failed Checks: assertion failed: gate_occupant
 ** 3 of 4 cover properties satisfied
VERIFICATION:- FAILED
Verification Time: 180.4s
'''


class SelectionTests(unittest.TestCase):
    def test_kernel_source_and_gate_changes_run_every_proof(self):
        for path in ['crates/kernel/src/lib.rs', 'crates/kernel/src/proofs.rs',
                     'crates/kernel/src/dependencies.rs', 'crates/kernel/Cargo.toml', 'Cargo.toml',
                     'tools/ci/kani_gate.py', 'tools/ci/kani-budgets.json',
                     '.github/workflows/gate.yml', '.github/workflows/kani-nightly.yml']:
            with self.subTest(path=path):
                self.assertEqual(kani_gate.triggered({path, 'docs/a.md'}), [path])

    def test_a_change_outside_the_kernel_runs_no_proof(self):
        for path in ['crates/controller/src/lib.rs', 'crates/kernel/README.md',
                     'crates/kernel/tests/scenarios.rs', 'crates/kernel/run.py',
                     'crates/kernel/tools/dependency_mutants.py', 'workers/src/tl3_workers/maker.py',
                     'Cargo.lock', 'docs/state-table.md']:
            with self.subTest(path=path):
                self.assertEqual(kani_gate.triggered({path}), [])

    def test_harnesses_are_read_from_the_proof_macro(self):
        source = 'proof!(a, check());\nproof!(\n    b_two,\n    3,\n    check()\n);\n  proof!(c, x);\n'
        self.assertEqual(kani_gate.harnesses(source), ['a', 'b_two'])
        self.assertEqual(kani_gate.harnesses(''), [])
        for broken in ['proof!(a, x);\nproof!(a, y);\n']:
            with self.assertRaises(kani_gate.KaniError):
                kani_gate.harnesses(broken)

    def test_every_kernel_harness_is_listed(self):
        names = kani_gate.harnesses()
        self.assertEqual(len(names), len(set(names)))

    def test_pr_mode_without_a_trigger_proves_nothing(self):
        with tempfile.TemporaryDirectory() as directory, \
             patch.object(kani_gate, 'changed_files', return_value={'workers/a.py'}), \
             patch.object(kani_gate, 'prove') as prove, \
             patch.dict(os.environ, {'GITHUB_STEP_SUMMARY': str(Path(directory, 'summary'))}):
            args = gate_args(directory)
            self.assertEqual(kani_gate.gate_main(args), 0)
            prove.assert_not_called()
            self.assertIn('no harness runs', Path(directory, 'summary').read_text())

    def test_pr_selects_only_measured_fast_tier_and_ignores_removed_harnesses(self):
        budgets = {'fast': {'seconds': 600, 'peak_gb': 1, 'tier': 'pr'},
                   'slow': {'seconds': 601, 'peak_gb': 1, 'tier': 'nightly'},
                   'removed': {'seconds': 1, 'peak_gb': 1, 'tier': 'pr'}}
        results = {'fast': {'name': 'fast', 'verdict': 'SUCCESSFUL'}}
        with tempfile.TemporaryDirectory() as directory, \
             patch.object(kani_gate, 'harnesses', return_value=['fast', 'slow']), \
             patch.object(kani_gate, 'load_budgets', return_value=budgets), \
             patch.object(kani_gate, 'changed_files', return_value={'crates/kernel/src/lib.rs'}), \
             patch.object(kani_gate, 'prove', return_value=(results, [])) as prove, \
             patch.dict(os.environ, {'BUILDBUDDY_API_KEY': 'fake'}):
            self.assertEqual(kani_gate.gate_main(gate_args(directory)), 0)
            self.assertEqual(prove.call_args.args[2], ['fast'])



class BudgetTests(unittest.TestCase):
    def test_budgets_cover_every_harness_with_a_measurement(self):
        budgets = kani_gate.load_budgets()
        self.assertTrue(set(kani_gate.harnesses()) <= set(budgets))
        for name, entry in budgets.items():
            with self.subTest(name=name):
                self.assertGreater(entry['seconds'], 0)
                self.assertGreater(entry['peak_gb'], 0)
                self.assertTrue(entry['invocation'].startswith('https://app.buildbuddy.io/invocation/'))
                self.assertLessEqual(entry['peak_gb'], kani_gate.MEMORY_GB - kani_gate.RESERVED_GB)

    def test_budget_builder_derives_tiers_and_pending_from_measurements(self):
        builder = getattr(kani_gate, 'build_budgets', None)
        self.assertTrue(callable(builder), 'missing reproducible budget builder')
        measured = {name: {'seconds': seconds, 'peak_gb': 1, 'invocation': 'https://run'}
                    for name, seconds in [('fast', 600), ('slow', 600.1),
                                          ('gate_leaf_progress_w2', 3301), ('gate_leaf_progress_w3', 3301)]}
        built = builder(measured)['harnesses']
        self.assertEqual(built['fast']['tier'], 'pr')
        self.assertEqual(built['slow']['tier'], 'nightly')
        for name in ('gate_leaf_progress_w2', 'gate_leaf_progress_w3'):
            self.assertEqual(built[name]['pending'], '#806')
            self.assertEqual(built[name]['tier'], 'nightly')
        built['slow']['seconds'] = 599
        self.assertEqual(builder(built)['harnesses']['slow']['tier'], 'pr')
        built['fast']['seconds'] = 601
        self.assertEqual(builder(built)['harnesses']['fast']['tier'], 'nightly')

    def test_committed_tiers_follow_the_measurements(self):
        for name, entry in kani_gate.load_budgets().items():
            with self.subTest(name=name):
                expected = 'pr' if entry['seconds'] <= 600 and not entry.get('lower_bound') else 'nightly'
                self.assertEqual(entry.get('tier'), expected)
        for name in ('gate_leaf_progress_w2', 'gate_leaf_progress_w3'):
            if name in kani_gate.load_budgets():
                self.assertEqual(kani_gate.load_budgets()[name].get('pending'), '#806')

    def test_pending_harnesses_are_attempted_alone_instead_of_rejected(self):
        budgets = {'pending': {'seconds': 3301, 'peak_gb': 34, 'pending': '#806'},
                   'fast': {'seconds': 10, 'peak_gb': 1}}
        try:
            shards = kani_gate.plan(list(budgets), budgets)
        except kani_gate.KaniError as error:
            self.fail(f'pending proof must be attempted: {error}')
        self.assertIn(['pending'], shards)
        self.assertEqual(sorted(sum(shards, [])), sorted(budgets))

    def test_timeouts_and_warnings_have_headroom(self):
        info = kani_gate.budget('a', {'a': {'seconds': 400.0, 'peak_gb': 8.0}})
        self.assertEqual(info['warn_after'], 600)
        self.assertEqual(info['timeout'], 1200)
        self.assertEqual(info['memory_gb'], 10)
        small = kani_gate.budget('a', {'a': {'seconds': 10.0, 'peak_gb': 0.5}})
        self.assertEqual((small['timeout'], small['memory_gb']), (kani_gate.MIN_TIMEOUT, 2))
        usable = kani_gate.MEMORY_GB - kani_gate.RESERVED_GB
        big = kani_gate.budget('a', {'a': {'seconds': 10.0, 'peak_gb': usable - 1.0}})
        self.assertEqual(big['memory_gb'], usable)
        unmeasured = kani_gate.budget('new', {})
        self.assertFalse(unmeasured['measured'])
        self.assertEqual(unmeasured['memory_gb'], kani_gate.MEMORY_GB - kani_gate.RESERVED_GB)

    def test_plan_balances_shards_and_keeps_every_harness_once(self):
        budgets = {f'p{i}': {'seconds': 10.0 * (i + 1), 'peak_gb': 4.0} for i in range(40)}
        shards = kani_gate.plan(sorted(budgets), budgets, cpus=4, memory_gb=44, max_shards=4, target=600)
        self.assertEqual(len(shards), 4)
        self.assertEqual(sorted(sum(shards, [])), sorted(budgets))
        loads = [sum(budgets[name]['seconds'] for name in shard) for shard in shards]
        self.assertLess(max(loads) - min(loads), 400)
        self.assertEqual(kani_gate.plan(['p0'], budgets), [['p0']])
        self.assertEqual(kani_gate.plan([], budgets), [])

    def test_every_shard_fits_inside_the_runner_hour(self):
        many = {f'p{i}': {'seconds': 1500.0, 'peak_gb': 35.0} for i in range(24)}
        shards = kani_gate.plan(sorted(many), many)
        self.assertEqual(sorted(sum(shards, [])), sorted(many))
        # Four 42 GB reservations share a 174 GB runner: two rounds of 1500 s
        # would pass the target, so each shard holds at most four.
        self.assertTrue(all(len(shard) <= 4 for shard in shards))
        for shard in shards:
            self.assertLessEqual(kani_gate.makespan(shard, many), kani_gate.TARGET_SECONDS)
        self.assertLessEqual(kani_gate.TARGET_SECONDS + kani_gate.SETUP_SECONDS,
                             kani_gate.RUNNER_MINUTES * 60)
        self.assertEqual(kani_gate.makespan(['a', 'b'], {'a': {'seconds': 10.0, 'peak_gb': 100.0},
                                                         'b': {'seconds': 20.0, 'peak_gb': 100.0}}), 30.0)

    def test_a_harness_longer_than_a_runner_hour_fails_the_plan(self):
        budgets = {'leaf': {'seconds': 3400.0, 'peak_gb': 14.0}, 'quick': {'seconds': 60.0, 'peak_gb': 2.0}}
        with self.assertRaisesRegex(kani_gate.KaniError, 'leaf takes 3400s.*Split the harness'):
            kani_gate.plan(['leaf', 'quick'], budgets)
        self.assertEqual(kani_gate.budget('leaf', budgets)['timeout'], kani_gate.PROVE_SECONDS)

    def test_plan_refuses_a_harness_no_runner_can_hold(self):
        with self.assertRaises(kani_gate.KaniError):
            kani_gate.plan(['huge'], {'huge': {'seconds': 1.0, 'peak_gb': kani_gate.MEMORY_GB + 1.0}})


class VerdictTests(unittest.TestCase):
    def test_only_a_complete_non_vacuous_proof_succeeds(self):
        self.assertEqual(kani_gate.verdict(SUCCESS, 0, False)['verdict'], 'SUCCESSFUL')
        result = kani_gate.verdict(SUCCESS, 0, False)
        self.assertEqual((result['line'], result['covers'], result['verification_seconds']),
                         ('VERIFICATION:- SUCCESSFUL', [2, 2], 12.5))
        cases = {
            'failed': (FAILURE, 1, False, 'VERIFICATION:- FAILED'),
            'exit code': (SUCCESS, 1, False, 'kani exited 1'),
            'no covers': (SUCCESS.replace(' ** 2 of 2 cover properties satisfied\n', ''), 0, False,
                          'no cover results reported'),
            'vacuous': (SUCCESS.replace('2 of 2', '1 of 2'), 0, False, 'may be vacuous'),
            'mixed': (SUCCESS + FAILURE, 1, False, 'VERIFICATION:- FAILED'),
        }
        for name, (output, code, timed_out, reason) in cases.items():
            with self.subTest(name):
                result = kani_gate.verdict(output, code, timed_out)
                self.assertEqual(result['verdict'], 'FAILED')
                self.assertIn(reason, result['reason'])
        self.assertEqual(kani_gate.verdict(SUCCESS, -9, True)['verdict'], 'TIMEOUT')
        self.assertEqual(kani_gate.verdict('killed', 137, False)['verdict'], 'KILLED')
        self.assertEqual(kani_gate.verdict(FAILURE, -9, True)['verdict'], 'FAILED')
        killed = kani_gate.verdict('Checking harness proofs::a...\nVERIFICATION:- FAILED\n', 137, False)
        self.assertEqual(killed['verdict'], 'FAILED')
        self.assertIn('likely out of memory', killed['reason'])


def fake_kani(output, code=0, sleep=0.0):
    """A stand-in for cargo kani that prints a fixed log."""
    script = f'import sys, time\ntime.sleep({sleep})\nsys.stdout.write({output!r})\nsys.exit({code})\n'
    return [sys.executable, '-c', script]


class ProverTests(unittest.TestCase):
    def prover(self, directory, commands, fail_fast=True, budgets=None, memory_gb=44, cpus=4):
        budgets = budgets or {name: {'seconds': 1.0, 'peak_gb': 1.0} for name in commands}
        return kani_gate.Prover(sorted(commands), budgets, Path(directory), cpus=cpus, memory_gb=memory_gb,
                                fail_fast=fail_fast, command=lambda name: commands[name], poll=0.01)

    def test_every_harness_reports_its_verdict_time_and_peak(self):
        with tempfile.TemporaryDirectory() as directory:
            results = self.prover(directory, {'a': fake_kani(SUCCESS), 'b': fake_kani(SUCCESS)}).run()
            self.assertEqual({name: r['verdict'] for name, r in results.items()},
                             {'a': 'SUCCESSFUL', 'b': 'SUCCESSFUL'})
            for result in results.values():
                self.assertGreater(result['peak_gb'], 0)
                self.assertGreaterEqual(result['seconds'], 0)
                self.assertEqual(result['line'], 'VERIFICATION:- SUCCESSFUL')
            self.assertIn('VERIFICATION:- SUCCESSFUL', Path(directory, 'a.log').read_text())

    def test_fail_fast_stops_the_shard_at_the_first_failure(self):
        commands = {'bad': fake_kani(FAILURE, 1), 'slow': fake_kani(SUCCESS, 0, sleep=30),
                    **{f'later{i}': fake_kani(SUCCESS) for i in range(8)}}
        with tempfile.TemporaryDirectory() as directory:
            budgets = {name: {'seconds': 1.0, 'peak_gb': 1.0} for name in commands}
            budgets['slow'] = {'seconds': 5.0, 'peak_gb': 1.0}
            budgets['bad'] = {'seconds': 3.0, 'peak_gb': 1.0}
            # Two slots: the slow harness and the failing one start first.
            results = self.prover(directory, commands, budgets=budgets, cpus=2).run()
            self.assertEqual(results['bad']['verdict'], 'FAILED')
            self.assertEqual(results['slow']['verdict'], 'STOPPED')
            self.assertTrue(any(r['verdict'] == 'NOT_RUN' for r in results.values()))
            self.assertEqual(sorted(results), sorted(commands))

    def test_nightly_proves_everything_after_a_failure(self):
        commands = {'bad': fake_kani(FAILURE, 1), **{f'later{i}': fake_kani(SUCCESS) for i in range(6)}}
        with tempfile.TemporaryDirectory() as directory:
            results = self.prover(directory, commands, fail_fast=False).run()
            self.assertEqual(sum(r['verdict'] == 'SUCCESSFUL' for r in results.values()), 6)
            self.assertEqual(results['bad']['verdict'], 'FAILED')

    def test_a_harness_past_its_hard_timeout_fails(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(kani_gate, 'MIN_TIMEOUT', 0.2):
            budgets = {'hang': {'seconds': 0.01, 'peak_gb': 1.0}}
            results = self.prover(directory, {'hang': fake_kani(SUCCESS, sleep=60)}, budgets=budgets).run()
            self.assertEqual(results['hang']['verdict'], 'TIMEOUT')

    def test_running_past_the_warning_budget_only_warns(self):
        with tempfile.TemporaryDirectory() as directory:
            budgets = {'a': {'seconds': 0.01, 'peak_gb': 1.0}}
            results = self.prover(directory, {'a': fake_kani(SUCCESS, sleep=0.5)}, budgets=budgets).run()
            self.assertEqual(results['a']['verdict'], 'SUCCESSFUL')
            self.assertTrue(results['a']['warning'])

    def test_memory_reservations_limit_parallel_harnesses(self):
        marker = '{}'
        script = ('import os, sys, time\n'
                  'path = sys.argv[1]\n'
                  'fd = os.open(path + "." + str(os.getpid()), os.O_CREAT | os.O_WRONLY)\n'
                  'time.sleep(0.3)\n'
                  'others = [p for p in os.listdir(os.path.dirname(path)) if p.startswith("run.")]\n'
                  'os.unlink(path + "." + str(os.getpid()))\n'
                  'sys.stdout.write(f"overlap {len(others)}\\n" + ' + repr(SUCCESS) + ')\n')
        with tempfile.TemporaryDirectory() as directory, tempfile.TemporaryDirectory() as scratch:
            commands = {name: [sys.executable, '-c', script, str(Path(scratch, 'run'))]
                        for name in ('a', 'b', 'c')}
            budgets = {name: {'seconds': 1.0, 'peak_gb': 16.0} for name in commands}
            results = self.prover(directory, commands, budgets=budgets, memory_gb=44).run()
            self.assertTrue(all(r['verdict'] == 'SUCCESSFUL' for r in results.values()))
            overlaps = [int(Path(directory, f'{name}.log').read_text().split()[1]) for name in commands]
            # 20 GB reservations: two fit in 42 usable GB, never three.
            self.assertLessEqual(max(overlaps), 2, marker)


class FakeClient:
    """Run API double: each Run gets an invocation; GetFile returns its shard record."""

    def __init__(self, records, running=()):
        self.records, self.runs, self.cancelled, self.running = records, [], [], set(running)

    def call(self, method, body, timeout=120, raw=False):
        if method == 'Run':
            self.runs.append(body)
            return {'invocationId': f'inv{len(self.runs) - 1}'}
        if method == 'GetInvocation':
            invocation_id = body['selector']['invocation_id']
            if invocation_id in self.running:
                return {'invocation': [{'invocationStatus': 'PARTIAL_INVOCATION_STATUS'}]}
            return {'invocation': [{'invocationStatus': 'COMPLETE_INVOCATION_STATUS', 'success': True,
                                    'artifacts': [{'name': kani_gate.RESULT, 'uri': invocation_id}]}]}
        if method == 'GetFile':
            index = int(body['uri'][3:])
            names = self.runs[index]['steps'][0]['run'].split('--index', 1)[1].split()[1:]
            return self.records(index, names)
        if method == 'CancelExecutions':
            self.cancelled.append(body['invocation_id'])
            return {}
        raise AssertionError(method)


def record(names, verdicts=None, head=HEAD, **fields):
    verdicts = verdicts or {}
    return json.dumps({'schema': kani_gate.SCHEMA, 'head': head, 'harnesses': names, 'error': None,
                       'results': {name: {'name': name, 'verdict': verdicts.get(name, 'SUCCESSFUL')}
                                   for name in names}, **fields}).encode()


def gate_args(directory, mode='pr'):
    class Args:
        pass
    args = Args()
    args.mode, args.base, args.head, args.branch = mode, 'b' * 40, HEAD, 'feature'
    args.repo, args.timeout, args.output_dir = 'https://github.com/o/r', 1, directory
    return args


class ControllerTests(unittest.TestCase):
    budgets = {f'p{i}': {'seconds': 600.0, 'peak_gb': 4.0} for i in range(24)}

    def prove(self, records, mode='pr', running=()):
        client = FakeClient(records, running)
        with tempfile.TemporaryDirectory() as directory, \
             patch.dict(os.environ, {'RUNNER_TEMP': directory}):
            results, errors = kani_gate.prove(client, gate_args(directory, mode), sorted(self.budgets),
                                              self.budgets, sleep=lambda _: None)
        return client, results, errors

    def test_shards_run_on_sized_remote_runners_and_all_pass(self):
        client, results, errors = self.prove(lambda index, names: record(names))
        self.assertEqual(errors, [])
        self.assertEqual(sorted(results), sorted(self.budgets))
        self.assertTrue(all(r['verdict'] == 'SUCCESSFUL' for r in results.values()))
        self.assertGreater(len(client.runs), 1)
        for run in client.runs:
            self.assertEqual(run['commit_sha'], HEAD)
            properties = run['platform_properties']
            self.assertEqual(properties['EstimatedCPU'], str(kani_gate.CPUS))
            self.assertEqual(properties['EstimatedMemory'], f'{kani_gate.MEMORY_GB}GB')
            self.assertEqual(properties['workload-isolation-type'], 'firecracker')
            self.assertNotIn('KEY', json.dumps(run).upper().replace('KANI', ''))

    def test_a_failed_harness_fails_and_cancels_the_other_shards(self):
        def records(index, names):
            return record(names, {names[0]: 'FAILED'} if index == 0 else {})
        client, results, errors = self.prove(records, running={f'inv{i}' for i in range(1, 8)})
        self.assertTrue(any(r['verdict'] == 'FAILED' for r in results.values()))
        self.assertGreater(len(client.runs), 1)
        # Only the runs still going are cancelled; their harnesses never pass.
        self.assertEqual(sorted(client.cancelled), sorted(f'inv{i}' for i in range(1, len(client.runs))))
        self.assertTrue(all(r['verdict'] == 'NOT_RUN' for r in results.values() if r['invocation'] != 'inv0'))

    def test_missing_wrong_or_partial_records_fail(self):
        cases = {
            'no record': lambda index, names: None,
            'not json': lambda index, names: b'{',
            'wrong head': lambda index, names: record(names, head='d' * 40),
            'other harnesses': lambda index, names: record(names[1:]),
            'shard error': lambda index, names: record(names, error='installing Kani 0.68.0 exited 1'),
        }
        for name, records in cases.items():
            with self.subTest(name):
                _, results, errors = self.prove(records, mode='nightly')
                self.assertTrue(errors)
                self.assertTrue(any(r['verdict'] != 'SUCCESSFUL' for r in results.values()))
        _, results, errors = self.prove(lambda index, names: json.dumps({
            'schema': kani_gate.SCHEMA, 'head': HEAD, 'harnesses': names, 'error': None,
            'results': {}}).encode(), mode='nightly')
        self.assertTrue(all(r['verdict'] == 'MISSING' for r in results.values()))

    def test_gate_main_fails_unless_every_harness_succeeds(self):
        names = sorted(self.budgets)
        outcomes = [({n: {'name': n, 'verdict': 'SUCCESSFUL'} for n in names}, [], 0),
                    ({n: {'name': n, 'verdict': 'SUCCESSFUL'} for n in names[1:]}, [], 1),
                    ({n: {'name': n, 'verdict': 'SUCCESSFUL'} for n in names}, ['lost'], 1)]
        for results, errors, expected in outcomes:
            with self.subTest(expected=expected, errors=errors, count=len(results)), \
                 tempfile.TemporaryDirectory() as directory, \
                 patch.object(kani_gate, 'harnesses', return_value=names), \
                 patch.object(kani_gate, 'load_budgets', return_value=self.budgets), \
                 patch.object(kani_gate, 'prove', return_value=(results, errors)), \
                 patch.dict(os.environ, {'BUILDBUDDY_API_KEY': 'fake'}), \
                 patch('sys.stdout', new_callable=io.StringIO):
                self.assertEqual(kani_gate.gate_main(gate_args(directory, 'nightly')), expected)
                saved = json.loads(Path(directory, 'kani-result.json').read_text())
                self.assertEqual(saved['passed'], expected == 0)

    def test_nightly_pending_timeouts_and_kills_do_not_hide_real_failures(self):
        names = ['pending', 'fast']
        budgets = {'pending': {'seconds': 3301, 'peak_gb': 34, 'pending': '#806', 'tier': 'nightly'},
                   'fast': {'seconds': 10, 'peak_gb': 1, 'tier': 'pr'}}
        cases = [('TIMEOUT', 0), ('KILLED', 0), ('FAILED', 1), ('MISSING', 1), ('SUCCESSFUL', 0)]
        for verdict, expected in cases:
            with self.subTest(verdict=verdict), tempfile.TemporaryDirectory() as directory, \
                 patch.object(kani_gate, 'harnesses', return_value=names), \
                 patch.object(kani_gate, 'load_budgets', return_value=budgets), \
                 patch.object(kani_gate, 'prove', return_value=({
                     'pending': {'name': 'pending', 'verdict': verdict},
                     'fast': {'name': 'fast', 'verdict': 'SUCCESSFUL'}}, [])), \
                 patch.dict(os.environ, {'BUILDBUDDY_API_KEY': 'fake'}):
                self.assertEqual(kani_gate.gate_main(gate_args(directory, 'nightly')), expected)
                summary = Path(directory, 'kani-summary.md').read_text()
                self.assertIn('pending #806', summary)
                self.assertIn(verdict, summary)


    def test_lost_pending_runner_requires_terminal_evidence_and_preserves_failure(self):
        class Client:
            def __init__(self, execution, text):
                self.execution, self.text = execution, text

            def call(self, method, body):
                if method == 'GetExecution':
                    return {'execution': [self.execution]}
                if method == 'GetLog':
                    return {'log': {'contents': self.text}}
                raise AssertionError(method)
        completed = {'stage': 'COMPLETED', 'status': {'code': 4}}
        for text, expected in [('', 'TIMEOUT'), (FAILURE, 'FAILED')]:
            result = kani_gate.pending_runner_result(Client(completed, text), 'inv', 'pending')
            self.assertEqual(result['verdict'], expected)
        self.assertIsNone(kani_gate.pending_runner_result(Client({'stage': 'EXECUTING'}, ''), 'inv', 'pending'))



class ShardTests(unittest.TestCase):
    def test_runner_entrypoint_scrubs_the_key_from_kani(self):
        seen = {}

        def run(command, env, **kwargs):
            seen[command[2]] = env
            return subprocess.CompletedProcess(command, 0)
        with tempfile.TemporaryDirectory() as directory, \
             patch.dict(os.environ, {'BUILDBUDDY_API_KEY': 'fake-runner-key',
                                     'BUILDBUDDY_ARTIFACTS_DIRECTORY': directory}), \
             patch.object(kani_gate.subprocess, 'run', side_effect=run):
            self.assertEqual(kani_gate.shard_entrypoint(['--head', HEAD]), 0)
        self.assertNotIn('BUILDBUDDY_API_KEY', seen['shard-child'])
        self.assertEqual(seen['-S']['BUILDBUDDY_API_KEY'], 'fake-runner-key')

    def test_a_dirty_or_other_checkout_is_refused(self):
        class Args:
            head, mode, index, harness = HEAD, 'pr', 0, ['apply_stop']
        with tempfile.TemporaryDirectory() as directory, \
             patch.dict(os.environ, {'BUILDBUDDY_ARTIFACTS_DIRECTORY': directory}), \
             patch.object(kani_gate.subprocess, 'run',
                          return_value=subprocess.CompletedProcess([], 0, stdout='d' * 40 + '\n')):
            self.assertEqual(kani_gate.shard_main(Args), 1)
            saved = json.loads(Path(directory, kani_gate.RESULT).read_text())
            self.assertIn('not ' + HEAD, saved['error'])


class ReportTests(unittest.TestCase):
    def test_report_dry_run_prints_the_issue_without_network_or_credentials(self):
        with tempfile.TemporaryDirectory() as directory, \
             patch.object(kani_gate.urllib.request, 'urlopen') as network, \
             patch('sys.stdout', new_callable=io.StringIO) as output:
            summary = Path(directory, 'nightly.md')
            summary.write_text('pending #806: TIMEOUT; regression: FAILED')
            try:
                code = kani_gate.main(['report', '--summary', str(summary), '--dry-run'])
            except SystemExit as error:
                self.fail(f'report needs a safe dry-run: {error}')
            self.assertEqual(code, 0)
            network.assert_not_called()
            self.assertIn(kani_gate.ISSUE_TITLE, output.getvalue())
            self.assertIn(summary.read_text(), output.getvalue())

    def test_a_regression_opens_one_issue_then_comments(self):
        calls = []

        def opener(existing):
            def open_(request, timeout):
                calls.append((request.get_method(), request.full_url,
                              json.loads(request.data) if request.data else None))
                if request.get_method() == 'GET':
                    payload = existing
                else:
                    payload = {'number': 7}
                return io.BytesIO(json.dumps(payload).encode())
            return open_
        self.assertEqual(kani_gate.report('table', 'o/r', 'fake', 'https://run', opener([])), 7)
        self.assertEqual(calls[-1][0:2], ('POST', 'https://api.github.com/repos/o/r/issues'))
        self.assertEqual(calls[-1][2]['title'], kani_gate.ISSUE_TITLE)
        self.assertIn('https://run', calls[-1][2]['body'])
        calls.clear()
        existing = [{'number': 3, 'title': kani_gate.ISSUE_TITLE}]
        self.assertEqual(kani_gate.report('table', 'o/r', 'fake', 'https://run', opener(existing)), 3)
        self.assertEqual(calls[-1][0:2], ('POST', 'https://api.github.com/repos/o/r/issues/3/comments'))


class WorkflowTests(unittest.TestCase):
    def setUp(self):
        self.gate = yaml.safe_load((ROOT / '.github/workflows/gate.yml').read_text())
        path = ROOT / '.github/workflows/kani-nightly.yml'
        self.nightly = yaml.safe_load(path.read_text()) if path.exists() else {}

    def test_the_required_rust_check_needs_the_kani_job(self):
        jobs = self.gate['jobs']
        self.assertIn('kani', jobs['rust-lint']['needs'])
        kani = jobs['kani']
        self.assertEqual(kani['if'], "github.event_name == 'pull_request'")
        runs = [step.get('run', '') for step in kani['steps']]
        self.assertTrue(any('tools/ci/kani_gate.py pr' in run for run in runs))
        cancel = next(step for step in kani['steps'] if 'kani_gate.py cancel' in step.get('run', ''))
        self.assertEqual(cancel['if'], 'cancelled()')
        # The job outlives every shard's runner timeout and its deadline.
        self.assertGreater(kani['timeout-minutes'], kani_gate.WAIT_MINUTES + 10)

    def test_nightly_proves_main_and_reports_a_regression(self):
        self.assertTrue(self.nightly, 'kani-nightly.yml is missing')
        triggers = self.nightly[True] if True in self.nightly else self.nightly['on']
        self.assertIn('schedule', triggers)
        job = self.nightly['jobs']['prove']
        runs = [step.get('run', '') for step in job['steps']]
        self.assertTrue(any('tools/ci/kani_gate.py nightly' in run for run in runs))
        self.assertGreater(job['timeout-minutes'], kani_gate.WAIT_MINUTES + 10)
        report = next(step for step in job['steps'] if 'kani_gate.py report' in step.get('run', ''))
        self.assertIn('failure()', report['if'])
        self.assertEqual(self.nightly['permissions']['issues'], 'write')


if __name__ == '__main__':
    unittest.main()
