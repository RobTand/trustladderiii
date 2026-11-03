import sys
import json
import os
import subprocess
import tempfile
import threading
from pathlib import Path
import unittest
from unittest.mock import patch

import rust_gate


class GateTests(unittest.TestCase):
    def assertBudgetWarnings(self, warnings, pattern):
        self.assertTrue(warnings)
        for warning in warnings:
            self.assertGreater(warning['measured'], warning['limit'])
        description = '\n'.join(
            f"{warning['test']}: {warning['metric'].replace('cpu', 'CPU').replace('_', ' ')} "
            f"{warning['measured']:.3f}s > {warning['limit']:.3f}s" for warning in warnings)
        self.assertRegex(description, pattern)

    def test_a_live_server_cannot_hide_another_fixture_that_disappeared(self):
        with tempfile.TemporaryDirectory() as temporary:
            fixture = Path(temporary, 'tl3-store-pg-live')
            (fixture / 'data').mkdir(parents=True)
            (fixture / 'fixture.pid').write_text('42')
            (fixture / 'data/postmaster.pid').write_text('99')
            with patch.dict(os.environ, {'TMPDIR': temporary}):
                sampler = rust_gate.PostgresCpu()
            with patch.object(rust_gate, 'process_cpu_sample', return_value=(1.0, 100)):
                self.assertEqual(sampler.sample(42), (1.0, ''))
                sampler.observe('embedded Postgres cached setup (fsync=true): 0.01s')
                delta, reason = sampler.sample(42)
            self.assertEqual(delta, 0)
            self.assertTrue(reason, 'a missing second postmaster must force a wall fallback')

    def test_captured_libtest_output_reports_a_missing_postgres_server(self):
        with tempfile.TemporaryDirectory() as temporary:
            executable = Path(temporary, 'fixture.py')
            event = {'type': 'test', 'event': 'ok', 'name': 'database', 'exec_time': .1,
                     'stdout': 'embedded Postgres cached setup (fsync=true): 0.01s\n'}
            summary = {'type': 'suite', 'event': 'ok', 'passed': 1}
            executable.write_text(f'print({json.dumps(event)!r}, flush=True)\n'
                                  f'print({json.dumps(summary)!r}, flush=True)\n')
            details = {}
            with patch.dict(os.environ, {'TMPDIR': temporary}):
                rust_gate.execute_binary({'name': 'fixture', 'command': [sys.executable, str(executable)]},
                                         False, cpu_details=details)
            self.assertEqual(set(details['cpu_fallbacks']), {'fixture::database'})

    def test_scenario_without_a_postgres_fixture_keeps_cpu_enforcement(self):
        with tempfile.TemporaryDirectory() as temporary:
            executable = Path(temporary, 'fixture.py')
            executable.write_text('print("test pure_logic ... ok", flush=True)\n'
                                  'print("1 fixture scenarios passed", flush=True)\n')
            details = {}
            with patch.dict(os.environ, {'TMPDIR': temporary}):
                rust_gate.execute_binary({'name': 'tl3-controller/prismabuild',
                                          'command': [sys.executable, str(executable)]},
                                         True, cpu_details=details)
            self.assertEqual(details['cpu_fallbacks'], {})

    def test_server_that_disappears_before_a_marker_falls_back_only_for_that_scenario(self):
        with tempfile.TemporaryDirectory() as temporary:
            executable = Path(temporary, 'fixture.py')
            executable.write_text('print("embedded Postgres cached setup (fsync=true): 0.01s", flush=True)\n'
                                  'print("test database ... ok", flush=True)\n'
                                  'print("test pure_logic ... ok", flush=True)\n'
                                  'print("2 fixture scenarios passed", flush=True)\n')
            details = {}
            with patch.dict(os.environ, {'TMPDIR': temporary}):
                rust_gate.execute_binary({'name': 'fixture', 'command': [sys.executable, str(executable)]},
                                         True, cpu_details=details)
            self.assertEqual(set(details['cpu_fallbacks']), {'fixture::database'})

    def test_live_postmaster_and_reaped_backend_cpu_reach_budget_enforcement(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fixture = root / 'tl3-store-pg-budget'
            (fixture / 'data').mkdir(parents=True)
            child = 'import time; start=time.process_time(); exec("while time.process_time()-start < 6: pass")'
            server = ('import subprocess, sys\n'
                      + f'subprocess.run([sys.executable, "-c", {child!r}], check=True)\n'
                      + 'print("ready", flush=True)\n'
                      + 'sys.stdin.readline()\n')
            with subprocess.Popen([sys.executable, '-c', server], stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE, text=True) as postmaster:
                try:
                    self.assertEqual(postmaster.stdout.readline(), 'ready\n')
                    (fixture / 'data/postmaster.pid').write_text(str(postmaster.pid))
                    executable = root / 'fixture.py'
                    executable.write_text('import os\nfrom pathlib import Path\n'
                                          + f'Path({str(fixture / "fixture.pid")!r}).write_text(str(os.getpid()))\n'
                                          + 'print("test query ... ok", flush=True)\n'
                                          + 'print("1 fixture scenarios passed", flush=True)\n')
                    cpu, details = {}, {}
                    with patch.dict(os.environ, {'TMPDIR': temporary}):
                        wall = rust_gate.execute_binary({'name': 'tl3-controller/publishing',
                                                         'command': [sys.executable, str(executable)]},
                                                        True, cpu_seconds=cpu, cpu_details=details)
                    self.assertGreaterEqual(sum(cpu.values()), 6)
                    self.assertGreaterEqual(sum(details['postgres_cpu_seconds'].values()), 6)
                    self.assertFalse(details['cpu_fallbacks'])
                    self.assertIsNone(postmaster.poll(), 'server must still be alive and unreaped')
                    key = 'tl3-controller/publishing::query'
                    server_cpu = details['postgres_cpu_seconds'][key]
                    test_cpu = cpu[key] - server_cpu
                    policy = self.cpu_policy()
                    policy['seconds'] = max(wall[key], 1) * 2
                    policy['cpu']['seconds'] = test_cpu + server_cpu / 2
                    rust_gate.check_budget({key: wall[key]}, policy, policy,
                                           cpu_seconds={key: test_cpu})
                    self.assertBudgetWarnings(rust_gate.check_budget({key: wall[key]}, policy, policy, cpu_seconds=cpu,
                                                   cpu_fallbacks=details['cpu_fallbacks']), 'CPU')
                finally:
                    postmaster.communicate('\n')

    def test_missing_postmaster_is_noted_and_uses_the_original_wall_budget(self):
        with tempfile.TemporaryDirectory() as temporary:
            fixture = Path(temporary, 'tl3-store-pg-missing')
            fixture.mkdir()
            executable = Path(temporary, 'fixture.py')
            executable.write_text('import os\nfrom pathlib import Path\n'
                                  + f'Path({str(fixture / "fixture.pid")!r}).write_text(str(os.getpid()))\n'
                                  + 'print("test query ... ok", flush=True)\n'
                                  + 'print("1 fixture scenarios passed", flush=True)\n')
            details = {}
            with patch.dict(os.environ, {'TMPDIR': temporary}):
                rust_gate.execute_binary({'name': 'tl3-controller/publishing',
                                          'command': [sys.executable, str(executable)]},
                                         True, cpu_details=details)
            key = 'tl3-controller/publishing::query'
            self.assertIn('postmaster.pid', details['cpu_fallbacks'][key])
            self.assertBudgetWarnings(rust_gate.check_budget({key: 30}, self.cpu_policy(), self.cpu_policy(),
                                           cpu_seconds={key: .1}, cpu_fallbacks=details['cpu_fallbacks']), 'wall 30.000s > 10.000s')

    def cpu_policy(self):
        return {'seconds': 10, 'slow': {'slow': 20},
                'cpu': {'seconds': 2, 'slow': {'slow': 4}},
                'wall_only': {'database': 'Work runs in a persistent server'},
                'wall_only_platforms': {'darwin': 'No Linux procfs', 'win32': 'No Linux procfs'}}

    def test_buildbuddy_stopgap_keeps_wall_only_scenarios_at_three_times_base(self):
        policy = self.cpu_policy()
        name = 'tl3-controller/workers::quota'
        policy['wall_only'] = {'tl3-controller/workers::*': 'Awaiting shard calibration'}
        for wall in [25, 30]:
            with self.subTest(wall=wall):
                rust_gate.check_budget({name: wall}, policy, policy,
                                       runner_class='buildbuddy-amd64', cpu_seconds={name: 100})
        self.assertBudgetWarnings(rust_gate.check_budget({name: 30.001}, policy, policy,
                                       runner_class='buildbuddy-amd64', cpu_seconds={name: 0}), 'wall 30.001s > 30.000s')

    def test_buildbuddy_stopgap_preserves_cpu_caps_and_tenfold_wall_backstop(self):
        policy = self.cpu_policy()
        policy['cpu']['seconds'] = 8
        self.assertBudgetWarnings(rust_gate.check_budget(
            {'scenario': 25}, policy, policy, runner_class='buildbuddy-amd64',
            cpu_seconds={'scenario': 8.001}), 'CPU 8.001s > 8.000s')
        rust_gate.check_budget({'scenario': 300}, policy, policy,
                               runner_class='buildbuddy-amd64', cpu_seconds={'scenario': 0})
        self.assertBudgetWarnings(rust_gate.check_budget({'scenario': 300.001}, policy, policy,
                                       runner_class='buildbuddy-amd64', cpu_seconds={'scenario': 0}), 'wall backstop 300.001s > 300.000s')

    def test_cpu_floor_covers_recorded_short_scenario_noise(self):
        policy = {'seconds': 10, 'slow': {}, 'cpu': {'seconds': 1, 'slow': {}}}
        name = 'tl3-controller/prismabuild::recorded_records_reach_kernel'
        for measured in [1.08, 1.14, 1.5, 1.56, 5]:
            with self.subTest(measured=measured):
                self.assertEqual(rust_gate.check_budget(
                    {name: 2}, policy, policy, cpu_seconds={name: measured}), [])
        self.assertBudgetWarnings(rust_gate.check_budget(
            {name: 2}, policy, policy, cpu_seconds={name: 5.001}), 'CPU 5.001s > 5.000s')

    def test_cpu_floor_keeps_regressions_and_larger_calibrated_caps_enforced(self):
        for calibrated, effective in [(1, 5), (8, 8)]:
            policy = {'seconds': 10, 'slow': {},
                      'cpu': {'seconds': 1, 'slow': {'scenario': calibrated}}}
            with self.subTest(calibrated=calibrated):
                self.assertEqual(rust_gate.check_budget(
                    {'scenario': 2}, policy, policy, cpu_seconds={'scenario': effective}), [])
                self.assertBudgetWarnings(rust_gate.check_budget(
                    {'scenario': 2}, policy, policy, cpu_seconds={'scenario': 3 * effective}),
                    f'CPU {3 * effective:.3f}s > {effective:.3f}s')

    def test_waiting_three_wall_budgets_passes_with_low_cpu(self):
        policy = self.cpu_policy()
        rust_gate.check_budget({'waiting': 30}, policy, policy, cpu_seconds={'waiting': 0.2})

    def test_cpu_and_wall_backstop_warn_independently(self):
        policy = self.cpu_policy()
        for wall, cpu, message in [(1, 6, 'CPU'), (100.01, .2, 'wall backstop')]:
            with self.subTest(message=message):
                self.assertBudgetWarnings(rust_gate.check_budget({'scenario': wall}, policy, policy,
                                           cpu_seconds={'scenario': cpu}), message)
        rust_gate.check_budget({'scenario': 100}, policy, policy, cpu_seconds={'scenario': 2})

    def test_cpu_limits_and_fallbacks_cannot_grow(self):
        baseline = self.cpu_policy()
        for change in [{'cpu': {'seconds': 3, 'slow': {}}},
                       {'cpu': {'seconds': 2, 'slow': {'slow': 5}}},
                       {'cpu': {'seconds': 2, 'slow': {'new': 1}}},
                       {'wall_only': {'new': 'Unreviewed exception'}},
                       {'wall_only_platforms': {'linux': 'Disable CPU checks'}}]:
            with self.subTest(change=change), self.assertRaisesRegex(RuntimeError, 'may not grow'):
                rust_gate.check_budget({}, {**baseline, **change}, baseline)
        with self.assertRaisesRegex(RuntimeError, 'may not grow'):
            rust_gate.check_budget({}, {**baseline, 'cpu': None}, baseline)

    def test_removing_a_cpu_override_below_default_cannot_grow_the_effective_limit(self):
        baseline = {'seconds': 10, 'slow': {}, 'cpu': {'seconds': 10, 'slow': {'fast': 1}}}
        with self.assertRaisesRegex(RuntimeError, 'may not grow'):
            rust_gate.check_budget({}, {**baseline, 'cpu': {'seconds': 10, 'slow': {}}}, baseline)

    def test_cpu_bootstrap_uses_cpu_units_and_still_preserves_wall_caps(self):
        old = {'seconds': 10, 'slow': {'slow': 20}}
        policy = {**old, 'cpu': {'seconds': 28, 'slow': {'slow': 36}}}
        rust_gate.check_budget({}, policy, old)
        with self.assertRaisesRegex(RuntimeError, 'may not grow'):
            rust_gate.check_budget({}, {**policy, 'seconds': 11}, old)
        with self.assertRaisesRegex(RuntimeError, 'may not grow'):
            rust_gate.check_budget({}, {**policy, 'cpu': {'seconds': 29, 'slow': {}}}, policy)

    def test_explicit_wall_only_fallbacks_and_missing_linux_samples(self):
        policy = self.cpu_policy()
        self.assertBudgetWarnings(rust_gate.check_budget({'database': 30}, policy, policy, cpu_seconds={'database': .1}), 'database')
        with self.assertRaisesRegex(RuntimeError, 'CPU.*missing|Missing CPU'):
            rust_gate.check_budget({'scenario': 1}, policy, policy, cpu_seconds={})
        with patch.object(sys, 'platform', 'darwin'):
            rust_gate.check_budget({'scenario': 10}, policy, policy, cpu_seconds={})
            self.assertBudgetWarnings(rust_gate.check_budget({'scenario': 30}, policy, policy, cpu_seconds={}), 'scenario')
        for cpu in [float('nan'), float('inf'), -1]:
            with self.subTest(cpu=cpu), self.assertRaisesRegex(RuntimeError, 'CPU'):
                rust_gate.check_budget({'scenario': 1}, policy, policy, cpu_seconds={'scenario': cpu})

    def test_proc_stat_handles_spaces_parentheses_and_all_four_cpu_counters(self):
        fields = ['S'] + ['0'] * 10 + ['100', '200', '300', '400'] + ['0'] * 20
        with patch.object(Path, 'read_text', return_value='42 (odd ) worker) ' + ' '.join(fields)), \
             patch.object(os, 'sysconf', return_value=100):
            self.assertEqual(rust_gate.process_cpu_seconds(42), 10)

    def test_custom_and_libtest_receipts_include_reaped_child_cpu_and_tail(self):
        with tempfile.TemporaryDirectory() as temporary:
            executable = Path(temporary, 'fixture.py')
            child = 'import time; start=time.process_time(); exec("while time.process_time()-start < .12: pass")'
            for custom in [True, False]:
                marker = ('test child ... ok' if custom else
                          json.dumps({'type': 'test', 'event': 'ok', 'name': 'child', 'exec_time': .5}))
                summary = ('1 fixture scenarios passed' if custom else
                           json.dumps({'type': 'suite', 'event': 'ok', 'passed': 1}))
                executable.write_text('import subprocess, sys\n'
                                      + f'subprocess.run([sys.executable, "-c", {child!r}], check=True)\n'
                                      + f'print({marker!r}, flush=True)\n'
                                      + f'print({summary!r}, flush=True)\n')
                cpu = {}
                results = rust_gate.execute_binary({'name': 'fixture',
                                                   'command': [sys.executable, str(executable)]},
                                                  custom, cpu_seconds=cpu)
                self.assertEqual(set(cpu), set(results))
                self.assertGreaterEqual(sum(cpu.values()), .1)

    def test_test_binary_does_not_inherit_runner_credentials(self):
        names = ['BUILDBUDDY_API_KEY', 'MERGIFY_TOKEN', 'GITHUB_TOKEN',
                 'OTHER_API_KEY', 'OTHER_TOKEN']
        code = ('import os, sys; '
                'sys.exit(1) if any(name.endswith(("_API_KEY", "_TOKEN")) '
                'for name in os.environ) else None; '
                'print(\'{"type":"suite","event":"ok","passed":0}\')')
        with patch.dict(os.environ, {name: 'synthetic-fixture' for name in names}):
            rust_gate.execute_binary({'name': 'credential-probe',
                                      'command': [sys.executable, '-c', code]}, False)

    def test_recorded_hosted_noise_is_quiet_and_regressions_warn(self):
        bridge = 'tl3-controller/tl3_controller::adapters::worktrees::tests::github_installation_and_synchronous_token_bridge'
        policy = {'seconds': 10, 'slow': {bridge: 15}}
        samples = {
            'hosted-x64': {'scope': 11.251069221, 'fixture routing': 10.36821593,
                           'effect_http_brief': 10.880650097},
            'hosted-arm64': {bridge: 15.343474895},
        }
        for runner, results in samples.items():
            with self.subTest(runner=runner):
                rust_gate.check_budget(results, policy, policy, runner_class=runner)
                self.assertBudgetWarnings(rust_gate.check_budget({name: policy['slow'].get(name, 10) * 2
                                                for name in results}, policy, policy,
                                               runner_class=runner), '.')
                self.assertBudgetWarnings(rust_gate.check_budget(results, policy, policy), '.')

    def test_hosted_limits_have_fixed_boundaries_and_cannot_mask_policy_growth(self):
        baseline = {'seconds': 10, 'slow': {'bridge': 15}}
        for runner, limit in [('hosted-x64', 12.5), ('hosted-arm64', 11.5)]:
            with self.subTest(runner=runner):
                rust_gate.check_budget({'fast': limit}, baseline, baseline, runner_class=runner)
                self.assertBudgetWarnings(rust_gate.check_budget({'fast': limit + .001}, baseline, baseline,
                                               runner_class=runner), 'fast')
                for policy in [{'seconds': 11, 'slow': {}},
                               {'seconds': 10, 'slow': {'bridge': 16}},
                               {'seconds': 10, 'slow': {'new': 1}}]:
                    with self.assertRaisesRegex(RuntimeError, 'may not grow'):
                        rust_gate.check_budget({}, policy, baseline, runner_class=runner)
        with self.assertRaisesRegex(ValueError, 'runner class'):
            rust_gate.check_budget({}, baseline, baseline, runner_class='unmeasured')

    def test_runner_class_uses_hosted_linux_metadata_and_keeps_fleet_strict(self):
        for environment, arch, expected in [('github-hosted', 'X64', 'hosted-x64'),
                                             ('github-hosted', 'ARM64', 'hosted-arm64'),
                                             ('self-hosted', 'ARM64', 'baseline'),
                                             ('', 'X64', 'baseline')]:
            with self.subTest(environment=environment, arch=arch), patch.dict(
                    os.environ, {'RUNNER_ENVIRONMENT': environment, 'RUNNER_OS': 'Linux',
                                 'RUNNER_ARCH': arch}):
                self.assertEqual(rust_gate.runner_class(), expected)
        with patch.dict(os.environ, {'RUNNER_ENVIRONMENT': 'github-hosted',
                                    'RUNNER_OS': 'Linux', 'RUNNER_ARCH': 'unknown'}):
            with self.assertRaisesRegex(ValueError, 'runner class'):
                rust_gate.runner_class()

    def test_hosted_workspace_records_class_and_preserves_functional_failures(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fixture = root / 'native.py'
            policy = root / rust_gate.POLICY
            policy.parent.mkdir(parents=True)
            policy.write_text(json.dumps({'seconds': 10, 'slow': {}}))
            valid = ('print(\'{"type":"test","event":"ok","name":"one","exec_time":11.251069221}\')\n'
                     'print(\'{"type":"suite","event":"ok","passed":1,"failed":0,"ignored":0}\')\n')
            fixture.write_text(valid)
            binary = {'name': 'fixture', 'custom': False,
                      'command': [sys.executable, str(fixture)]}
            with patch.object(rust_gate, 'ROOT', root), \
                 patch.object(rust_gate, 'baseline_policy', return_value={'seconds': 10, 'slow': {}}), \
                 patch.object(rust_gate, 'collect_binaries', return_value=[binary]), \
                 patch.dict(os.environ, {'RUNNER_ENVIRONMENT': 'github-hosted',
                                         'RUNNER_OS': 'Linux', 'RUNNER_ARCH': 'X64'}):
                report = rust_gate.test_workspace(Path('Cargo.toml'))
                self.assertEqual(report['runner_class'], 'hosted-x64')
                self.assertEqual(report['budget_multiplier'], 1.25)
                self.assertEqual(report['tests']['fixture::one'], 11.251069221)
                fixture.write_text(valid.replace('11.251069221', '56.0'))
                report = rust_gate.test_workspace(Path('Cargo.toml'))
                expected = [{'test': 'fixture::one', 'measured': 56.0, 'limit': 12.5, 'metric': 'wall'}]
                self.assertEqual(report['budget_warnings'], expected)
                saved = json.loads((root / 'target/ci-test-times.json').read_text())
                self.assertEqual(saved['budget_warnings'], expected)
                for content, message in [(valid + 'raise SystemExit(1)\n', 'exit'),
                                         (valid.replace('"event":"ok","name"',
                                                        '"event":"ignored","name"'), 'skipped')]:
                    fixture.write_text(content)
                    with self.subTest(message=message), self.assertRaisesRegex(RuntimeError, message):
                        rust_gate.test_workspace(Path('Cargo.toml'))

    def test_shallow_pr_checkout_uses_target_policy_instead_of_its_own_exceptions(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / 'source'
            checkout = Path(temporary) / 'checkout'
            source.mkdir()

            def git(*args, cwd=source):
                return subprocess.check_output(['git', '-c', 'user.name=Fixture',
                                                '-c', 'user.email=fixture@example.invalid',
                                                '-c', 'commit.gpgsign=false', *args],
                                               cwd=cwd, text=True).strip()

            git('init', '--quiet', '-b', 'main')
            policy = source / rust_gate.POLICY
            policy.parent.mkdir(parents=True)
            original = {'seconds': 2, 'slow': {'contract': 10}}
            policy.write_text(json.dumps(original))
            git('add', '.')
            git('commit', '--quiet', '-m', 'Target policy')
            base = git('rev-parse', 'HEAD')
            git('checkout', '--quiet', '-b', 'feature')
            enlarged = {'seconds': 2, 'slow': {'new': 100}}
            policy.write_text(json.dumps(enlarged))
            git('commit', '--quiet', '-am', 'PR attempts to add an exception')
            git('clone', '--quiet', '--depth=1', '--branch=feature', source.as_uri(), str(checkout))
            event = Path(temporary) / 'event.json'
            event.write_text(json.dumps({'pull_request': {'base': {'sha': base}}}))
            with patch.object(rust_gate, 'ROOT', checkout), patch.dict(os.environ, {'GITHUB_EVENT_PATH': str(event)}):
                baseline = rust_gate.baseline_policy(enlarged)
            self.assertEqual(baseline, original)
            with self.assertRaisesRegex(RuntimeError, 'grow'):
                rust_gate.check_budget({}, enlarged, baseline)

    def test_budget_warns_for_slow_tests_without_an_exception(self):
        config = {'seconds': 2, 'slow': {'worker::contract': 10}}
        rust_gate.check_budget({'worker::contract': 3, 'fast::one': 1}, config, config)
        self.assertBudgetWarnings(rust_gate.check_budget({'fast::one': 3}, config, config), 'fast::one')
        self.assertBudgetWarnings(rust_gate.check_budget({'worker::contract': 11}, config, config), 'worker::contract')

    def test_slow_list_and_budgets_can_only_shrink(self):
        baseline = {'seconds': 2, 'slow': {'contract': 10, 'removed': 20}}
        rust_gate.check_budget({}, {'seconds': 1, 'slow': {'contract': 8}}, baseline)
        for config in [
            {'seconds': 3, 'slow': {}},
            {'seconds': 2, 'slow': {'contract': 11}},
            {'seconds': 2, 'slow': {'new': 10}},
        ]:
            with self.subTest(config=config), self.assertRaisesRegex(RuntimeError, 'grow'):
                rust_gate.check_budget({}, config, baseline)

    def test_libtest_reports_actual_test_times_and_failed_exits_fail(self):
        with tempfile.TemporaryDirectory() as temporary:
            executable = Path(temporary) / 'fixture.py'
            executable.write_text(
                'import json, sys\n'
                'print(json.dumps({"type":"test", "event":"ok", "name":"one", "exec_time":0.25}))\n'
                'print(json.dumps({"type":"suite", "event":"ok", "passed":1, "failed":0, "ignored":0}))\n'
                'sys.exit(1 if "fail" in sys.argv else 0)\n'
            )
            binary = {'name': 'fixture', 'command': [sys.executable, str(executable)]}
            results = rust_gate.execute_binary(binary, custom=False)
            self.assertEqual(results, {'fixture::one': 0.25})
            with self.assertRaisesRegex(RuntimeError, 'exit'):
                rust_gate.execute_binary(binary, custom=False, arguments=('fail',))

    def test_custom_harness_measures_intervals_and_includes_unreported_tail(self):
        with tempfile.TemporaryDirectory() as temporary:
            executable = Path(temporary) / 'fixture.py'
            executable.write_text('import sys\nsys.stderr.write("partial diagnostic: ")\n'
                                  'sys.stderr.flush()\nprint("test named_scenario ... ok")\n'
                                  'print("1 fixture scenarios passed; no skips")\n')
            binary = {'name': 'fixture', 'command': [sys.executable, str(executable)]}
            results = rust_gate.execute_binary(binary, custom=True)
            self.assertIn('fixture::named_scenario', results)
            self.assertIn('fixture::__tail__', results)
            self.assertGreaterEqual(results['fixture::named_scenario'], 0)

    def test_custom_harness_refuses_fewer_or_more_markers_than_declared(self):
        for declared, actual in [(3, 2), (1, 2), (1, 0)]:
            with self.subTest(declared=declared, actual=actual), tempfile.TemporaryDirectory() as temporary:
                executable = Path(temporary) / 'fixture.py'
                executable.write_text(''.join(f'print("test scenario_{i} ... ok")\n'
                                              for i in range(actual))
                                      + f'print("{declared} fixture scenarios passed; no skips")\n')
                binary = {'name': 'fixture/custom', 'command': [sys.executable, str(executable)]}
                with self.assertRaisesRegex(RuntimeError,
                                            f'fixture/custom:.*declared {declared}.*received {actual}'):
                    rust_gate.execute_binary(binary, custom=True)

    def test_computed_shard_summary_still_rejects_missing_or_duplicated_scenarios(self):
        planned = ['first', 'second', 'third', 'added_independently']
        for emitted in [planned[:-1], planned[:-1] + [planned[0]]]:
            with self.subTest(emitted=emitted), tempfile.TemporaryDirectory() as temporary:
                executable = Path(temporary, 'fixture.py')
                executable.write_text(''.join(f'print("test {name} ... ok")\n' for name in emitted)
                                      + f'print("{len(planned)} fixture scenarios passed; no skips")\n')
                with self.assertRaisesRegex(RuntimeError, 'declared|duplicate'):
                    rust_gate.execute_binary({'name': 'fixture/shard',
                                              'command': [sys.executable, str(executable)]}, custom=True)

    def test_custom_harness_requires_one_scenario_count_declaration(self):
        for summaries in [[], ['1 fixture scenarios passed'] * 2]:
            with self.subTest(summaries=summaries), tempfile.TemporaryDirectory() as temporary:
                executable = Path(temporary) / 'fixture.py'
                executable.write_text('print("test one ... ok")\n'
                                      + ''.join(f'print({summary!r})\n' for summary in summaries))
                with self.assertRaisesRegex(RuntimeError, 'fixture/custom:.*scenario count'):
                    rust_gate.execute_binary({'name': 'fixture/custom',
                                              'command': [sys.executable, str(executable)]}, custom=True)

    def test_custom_harness_duplicate_markers_cannot_hide_an_unmarked_case(self):
        with tempfile.TemporaryDirectory() as temporary:
            executable = Path(temporary) / 'fixture.py'
            executable.write_text('print("test one ... ok")\n' * 2
                                  + 'print("2 fixture scenarios passed; no skips")\n')
            with self.assertRaisesRegex(RuntimeError, 'fixture/custom:.*duplicate.*one'):
                rust_gate.execute_binary({'name': 'fixture/custom',
                                          'command': [sys.executable, str(executable)]}, custom=True)

    def test_different_binaries_overlap_and_worker_count_is_bounded(self):
        barrier = threading.Barrier(2, timeout=5)
        active = 0
        peak = 0
        lock = threading.Lock()

        def execute(binary, custom, cpu_seconds=None, cpu_details=None):
            nonlocal active, peak
            with lock:
                active += 1
                peak = max(peak, active)
            barrier.wait()
            with lock:
                active -= 1
            return {binary['name']: 0}

        binaries = [{'name': str(i), 'custom': False} for i in range(4)]
        with patch.object(rust_gate, 'execute_binary', side_effect=execute):
            results = rust_gate.execute_binaries(binaries, jobs=2)
        self.assertEqual(peak, 2)
        self.assertEqual(set(results), {'0', '1', '2', '3'})

    def test_missing_native_results_and_skipped_tests_fail(self):
        for content in [
            'print("unrelated output")',
            'print(\'{"type":"test","event":"ignored","name":"missing"}\')',
            'print(\'{"type":"suite","event":"ok","passed":1}\')',
        ]:
            with tempfile.TemporaryDirectory() as temporary:
                path = Path(temporary) / 'fixture.py'
                path.write_text(content)
                with self.assertRaisesRegex(RuntimeError, 'suite|skipped'):
                    rust_gate.execute_binary(
                        {'name': 'fixture', 'command': [sys.executable, str(path)]}, custom=False,
                    )


if __name__ == '__main__':
    unittest.main()
