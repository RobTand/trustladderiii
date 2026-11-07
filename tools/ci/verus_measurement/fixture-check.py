"""Admitted-only producer fixtures. No Bazel, Verus or real scanner execution."""
import base64
import copy
import io
import json
import os
from pathlib import Path
import runpy
import stat
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

HERE = Path(__file__).resolve().parent
S = runpy.run_path(str(HERE / 'submit.py'))
D = S['D']
HEAD = subprocess.check_output(['git', '-C', str(HERE.parents[2]), 'rev-parse', 'HEAD'], text=True).strip()


def configuration():
    return {'head': HEAD, 'comparison_base': '1' * 40, 'branch': 'fixture/measurement',
            'mode': 'normal', 'watchdog_seconds': 45, 'outer_seconds': 180,
            'coordinator_minutes': 30, 'diagnostic_authorization': None, 'raw_queries': False,
            'selections': [{'target': '//crates/kernel:verus_dependencies',
                            'selector': 'kernel::dependencies#World::record_dependency', 'expected': 1}]}


def fake_execution(row, mode='normal'):
    tools = {key: {'path': path, 'sha256': 'a' * 64, 'bytes': 1} for key, path in
             [('verifier', '/FAKE/verus'), ('rustc', '/FAKE/rust/bin/rustc'), ('z3', '/FAKE/z3'), ('rust_verify', '/FAKE/rust_verify')]}
    tools['z3']['sha256'] = D['source_pins'](HERE.parents[2])['z3']
    owner, _, function = row.selector.partition('#')
    argv = ['/usr/bin/timeout', '--kill-after=5s', '120s' if mode == 'diagnostic' else '45s', '/FAKE/verus',
            '--num-threads', '1', '--rlimit', '30', '--multiple-errors', '0', '-V', 'spinoff-all',
            '--sysroot', '/FAKE/rust', 'crates/kernel/verus/progress.rs', '--edition', '2024', '--verify-only-module', owner,
            '--time-expanded', '--output-json']
    if function:
        argv += ['--verify-function', function]
    if mode == 'frontend-only':
        argv += ['--no-verify']
    return {'protocol': D['CAPABILITY'], 'mode': mode, 'argv': argv, 'tools': tools, 'pins': D['source_pins'](HERE.parents[2]), 'sysroot': '/FAKE/rust', 'status': 0,
            'environment': {'PATH': '/FAKE/rust/bin:/usr/bin:/bin', 'LD_LIBRARY_PATH': '/FAKE/rust/lib',
                'VERUS_USE_RUSTUP': '0', 'VERUS_Z3_PATH': '/FAKE/z3', 'OMP_NUM_THREADS': '1', 'RAYON_NUM_THREADS': '1'}}


def fake_wrapper():
    # Expressly fake producer verdict, never retained RP/scanner evidence.
    return {'schema': 'tl3.bbrun.wrapper-result', 'version': 1, 'parent_invocation_id': None,
            'command': {'status': 'pass', 'exit_code': 0, 'reason': 'command_succeeded'},
            'artifact_handoff': {'status': 'pass', 'reason': 'handoff_succeeded'},
            'scanner': {'status': 'pass', 'first_exit_code': 9, 'exit_code': 0, 'reason': 'scan_clean'},
            'cleanup': {'status': 'not_needed', 'targets_present': False, 'reason': 'no_cleanup_targets'},
            'wrapper': {'original_exit_code': 0, 'exit_code': 0}}


class Fixtures(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory(prefix='measurement-fixture-', dir=Path.cwd())
        cls.root = Path(cls.temp.name)
        cls.source = cls.root / 'source'
        cls.evidence = cls.root / 'evidence'
        cls.evidence.mkdir(mode=0o700)
        subprocess.run(['git', 'init', '-q', str(cls.source)], check=True)
        subprocess.run(['git', '-C', str(cls.source), 'fetch', '-q', '--depth=1',
                        str(HERE.parents[2]), HEAD], check=True)
        subprocess.run(['git', '-C', str(cls.source), 'checkout', '-q', '--detach', 'FETCH_HEAD'], check=True)
        assert D['git'](cls.source, 'rev-parse', '--is-shallow-repository').strip() == b'true'
        cls.original = (cls.source / D['RUNNER']).read_bytes()

    def config(self, **changes):
        return D['Config'].parse({**configuration(), **changes})

    def test_full_preparation_and_receipts(self):
        for mode in ('normal', 'diagnostic', 'frontend-only'):
            config = self.config(mode=mode, watchdog_seconds=120 if mode == 'diagnostic' else 45,
                                 diagnostic_authorization='FAKE fixture authorization' if mode == 'diagnostic' else None)
            receipt = self.evidence / f'{mode}-receipt.json'
            command = S['submission'](config, self.evidence / 'runner.log')
            result = S['full_dry_run'](self.source, config, self.evidence / mode, receipt, command)
            S['check_receipt'](self.source, config, receipt, command)
            self.assertEqual(result['full_preparation'], 'PASS')
            preview = (self.evidence / mode / 'runner-preview.rs').read_text()
            self.assertEqual(preview.encode(), self.original)
            self.assertEqual((self.evidence / mode / 'runner.patch').read_bytes(), b'')
            self.assertIn('Toolchain::new', preview)
            self.assertIn('--test_env=TL3_VERUS_RAW_QUERIES=0', config.command())
            self.assertEqual((self.source / D['RUNNER']).read_bytes(), self.original)
            with self.assertRaisesRegex(ValueError, 'exact_submission_argv'):
                S['check_receipt'](self.source, config, receipt, command + ['stale'])
        receipt = self.evidence / 'invalidated-receipt.json'
        receipt.write_text('{}')
        with self.assertRaises(FileExistsError):
            S['full_dry_run'](self.source, self.config(), self.evidence / 'normal', receipt, [])
        self.assertFalse(receipt.exists())
        with self.assertRaisesRegex(ValueError, 'missing successful'):
            S['check_receipt'](self.source, self.config(), receipt, [])
        bad = configuration()
        bad['selections'][0]['expected'] = 99
        receipt.write_text('{}')
        with self.assertRaisesRegex(ValueError, 'selector/count'):
            config = D['Config'].parse(bad)
            S['full_dry_run'](self.source, config, self.evidence / 'failed-count', receipt,
                              S['submission'](config, self.evidence / 'runner.log'))
        self.assertFalse(receipt.exists())

    def test_config_and_inventory(self):
        for change in ({'watchdog_seconds': 120}, {'outer_seconds': 45}, {'raw_queries': 1},
                       {'head': 'main'}, {'mode': 'diagnostic', 'watchdog_seconds': 120},
                       {'selections': configuration()['selections'] * 2}):
            with self.assertRaises(ValueError):
                self.config(**change)
        for key, value in (('expected', True), ('expected', 2), ('selector', 'wrong#selector')):
            data = configuration()
            data['selections'][0][key] = value
            with self.assertRaises(ValueError):
                cfg = D['Config'].parse(data)
                D['prepare'](self.source, cfg, D['owned_directory'](self.root / f'bad-{key}-{value}'))
        with self.assertRaisesRegex(ValueError, 'duplicate JSON'):
            D['loads']('{"a": 1, "a": 2}')
        with self.assertRaises(ValueError):
            D['source_identity'](self.source, self.config(head='2' * 40))
        extra = self.source / 'unexpected'
        extra.write_text('dirty')
        try:
            with self.assertRaisesRegex(ValueError, 'clean'):
                D['source_identity'](self.source, self.config())
        finally:
            extra.unlink()

    def test_anchors_and_restoration(self):
        text = self.original.decode()
        with self.assertRaisesRegex(ValueError, 'unsupported runner capability'):
            D['instrument'](text.replace(D['CAPABILITY'], 'old-runner'), self.config())
        changed = D['instrument'](text, self.config()).encode()
        self.assertEqual(changed, self.original)
        with self.assertRaisesRegex(RuntimeError, 'fixture exception'):
            with D['disposable_runner'](self.source, self.original, changed):
                self.assertEqual((self.source / D['RUNNER']).read_bytes(), changed)
                raise RuntimeError('fixture exception')
        self.assertEqual((self.source / D['RUNNER']).read_bytes(), self.original)
        self.assertEqual(D['instrument'](text, self.config(raw_queries=True)), text)

    def test_curated_readonly_and_metrics(self):
        source = self.root / 'read-only.json'
        source.write_bytes(b'{"fixture":true}\n')
        source.chmod(0o444)  # Owned fixture only, never Bazel/CAS/source modes.
        output = D['owned_directory'](self.root / 'staging')
        copied = D['put'](output, 'curated.json', D['regular_bytes'](source))
        self.assertEqual(copied['sha256'], D['digest'](source.read_bytes()))
        self.assertEqual(stat.S_IMODE((output / 'curated.json').stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE(output.stat().st_mode), 0o700)
        self.assertEqual(stat.S_IMODE(source.stat().st_mode), 0o444)
        link = self.root / 'link'
        link.symlink_to(source)
        with self.assertRaises(OSError):
            D['regular_bytes'](link)
        fifo = self.root / 'fifo'
        os.mkfifo(fifo)
        with self.assertRaises(ValueError):
            D['regular_bytes'](fifo)
        row = self.config().selections[0]
        stats = {'verification-results': {'verified': 1, 'errors': 0, 'encountered-error': False,
                 'encountered-vir-error': False, 'is-verifying-entire-crate': False},
                 'times-ms': {'total': 0, 'num-threads': 1, 'smt': {'rlimit-run': 0, 'rlimit-init': 7, 'smt-run-module-times': [{'function-breakdown': [{'rlimit': 0, 'success': True}]}]}}}
        proof = {'proofs': {row.target: {'kind': 'verus', 'module': row.selector, 'verified': 1,
                  'expected_obligations': 1, 'threads': 1, 'seconds': 0}}}
        measurement = {'label': row.target, 'verified': 1, 'seconds': 0, 'rlimit_budget': 30,
                       'rlimit_run': 0, 'rlimit_init': 7, 'function_rlimit_sum': 0, 'function_count': 1, 'max_function_rlimit': 0}
        self.assertEqual(D['child_verdict'](stats, proof, measurement, row, execution=fake_execution(row))['resources'], 'PASS')
        del stats['times-ms']['total']
        self.assertEqual(D['child_verdict'](stats, proof, measurement, row, execution=fake_execution(row))['resources'], 'UNKNOWN')
        stats['verification-results']['success'] = False
        with self.assertRaises(ValueError):
            D['child_verdict'](stats, proof, measurement, row, execution=fake_execution(row))
        stats['verification-results']['success'] = True
        stats['verification-results']['verified'] = True
        with self.assertRaises(ValueError):
            D['child_verdict'](stats, proof, measurement, row, execution=fake_execution(row))

    def test_frontend_boundaries_and_prerequisite(self):
        row = self.config().selections[0]
        stats = {'verification-results': {'verified': 0, 'errors': 0, 'encountered-error': False,
                 'encountered-vir-error': False, 'is-verifying-entire-crate': False},
                 'times-ms': {'smt': {**{k: 0 for k in ('total', 'smt-init', 'smt-run', 'rlimit-init', 'rlimit-run')},
                     'smt-init-module-times': [], 'smt-run-module-times': []}}}
        proof = {'proofs': {row.target: {'kind': 'FRONTEND_ONLY', 'module': row.selector, 'verified': 0,
                 'expected_obligations': row.expected, 'threads': 1, 'seconds': 1}}}
        metrics = {'label': row.target, 'mode': 'FRONTEND_ONLY', 'no_smt': True,
                   'proof': 'NOT_RUN', 'resource_acceptance': 'NOT_APPLICABLE'}
        execution = fake_execution(row, 'frontend-only')
        result = D['child_verdict'](stats, proof, metrics, row, 'frontend-only', execution)
        self.assertEqual(result['proof'], 'NOT_RUN')
        self.assertEqual(result['resources'], 'NOT_APPLICABLE')
        for key in ('rlimit-run', 'rlimit-init', 'smt-run-module-times'):
            bad = copy.deepcopy(stats)
            del bad['times-ms']['smt'][key]
            with self.assertRaises(ValueError):
                D['child_verdict'](bad, proof, metrics, row, 'frontend-only', execution)
        for key, value in [('verified', 1), ('verified', True), ('errors', 1), ('success', False), ('encountered-error', True)]:
            bad = copy.deepcopy(stats)
            bad['verification-results'][key] = value
            with self.assertRaises(ValueError):
                D['child_verdict'](bad, proof, metrics, row, 'frontend-only', execution)
        for mutate in [lambda e: e.update(status=127), lambda e: e.update(mode='normal'),
                       lambda e: e.update(sysroot='/FAKE/rust/bin/rustc'),
                       lambda e: e['argv'].append('--build-sst'), lambda e: e['argv'].remove('--no-verify')]:
            bad = copy.deepcopy(execution)
            mutate(bad)
            with self.assertRaises(ValueError):
                D['child_verdict'](stats, proof, metrics, row, 'frontend-only', bad)
        with self.assertRaisesRegex(ValueError, 'missing frontend prerequisite'):
            S['check_frontend'](self.source, self.config(), None, self.evidence)
        locator = self.evidence / 'fake-frontend-locator.json'
        for prior in [{'overall': 'FAIL'}, {'overall': 'PASS', 'config': self.config().json()}]:
            locator.write_bytes(D['encoded'](prior))
            with self.assertRaisesRegex(ValueError, 'failed/stale'):
                S['check_frontend'](self.source, self.config(), locator, self.evidence)

    def test_curated_archive_boundary(self):
        directory = self.root / 'archive-child'
        directory.mkdir()
        outputs = directory / 'test.outputs'
        outputs.mkdir()
        archive = outputs / 'outputs.zip'
        with zipfile.ZipFile(archive, 'w') as zipped:
            zipped.writestr('verifier.json', b'{"FAKE":true}')
            zipped.writestr('AIR/unused.air', b'not ordinarily collected')
        archive.chmod(0o444)
        outputs.chmod(0o555)  # Newly owned fake input tree only.
        try:
            self.assertEqual(D['output_bytes'](directory, 'verifier.json'), b'{"FAKE":true}')
            staged = D['owned_directory'](self.root / 'archive-staging')
            D['put'](staged, 'verifier.json', D['output_bytes'](directory, 'verifier.json'))
            self.assertEqual([p.name for p in staged.iterdir()], ['verifier.json'])
            self.assertEqual(stat.S_IMODE(archive.stat().st_mode), 0o444)
        finally:
            outputs.chmod(0o700)
        archive.unlink()
        with zipfile.ZipFile(archive, 'w') as zipped:
            link = zipfile.ZipInfo('verifier.json')
            link.external_attr = (stat.S_IFLNK | 0o777) << 16
            zipped.writestr(link, 'outside')
        with self.assertRaisesRegex(ValueError, 'unsafe zip'):
            D['output_bytes'](directory, 'verifier.json')

    def test_fake_wrapper_v1_consumer(self):
        parent = '12345678-1234-1234-1234-123456789abc'
        wrapper = fake_wrapper()
        args = {'parent': parent, 'terminal': 'PASS', 'trusted_artifact': json.dumps(wrapper)}
        good = S['wrapper_evidence'](**args)
        driver = {k: 'PASS' for k in ('preparation', 'child', 'resources', 'collection', 'restoration')}
        self.assertEqual(S['qualify'](driver, good, 'PASS'), 'PASS')
        self.assertEqual(S['qualify'](driver, good, 'FAIL'), 'FAIL')
        self.assertEqual(S['qualify'](driver, None, 'PASS'), 'UNKNOWN')
        self.assertEqual(S['qualify']({**driver, 'resources': 'UNKNOWN'}, good, 'PASS'), 'UNKNOWN')
        self.assertIsNone(S['wrapper_evidence'](parent=parent, terminal='FAIL'))
        self.assertEqual(S['wrapper_evidence'](**args, trusted_runner_line=S['MARKER'] + json.dumps(wrapper)), good)
        for path, value in [(('version',), True), (('version',), 2), (('extra',), 1),
                            (('parent_invocation_id',), 'bad'), (('command', 'exit_code'), True),
                            (('scanner', 'exit_code'), 4), (('cleanup', 'targets_present'), True),
                            (('cleanup', 'reason'), 'untrusted_secret'), (('wrapper', 'exit_code'), True)]:
            bad = copy.deepcopy(wrapper)
            target = bad
            for key in path[:-1]:
                target = target[key]
            target[path[-1]] = value
            with self.subTest(path=path), self.assertRaises(ValueError):
                S['wrapper_evidence'](**{**args, 'trusted_artifact': json.dumps(bad)})
        for extra in ({'terminal': 'UNKNOWN'}, {'command_reserved_collision': True},
                      {'trusted_runner_line': S['MARKER'] + json.dumps(wrapper) + '\n' + S['MARKER'] + '{}'},
                      {'trusted_runner_line': S['MARKER'] + json.dumps({**wrapper, 'version': 2})}):
            with self.assertRaises(ValueError):
                S['wrapper_evidence'](**{**args, **extra})
        self.evidence.joinpath('fake-wrapper-v1.json').write_text(json.dumps(wrapper, indent=2))
        self.evidence.joinpath('fixture-separation.txt').write_text('FAKE producer interface fixtures only. No scanner execution or retained RP qualification.\n')

    def test_same_invocation_and_hold(self):
        parent = '12345678-1234-1234-1234-123456789abc'
        verdict = 'verdict: ' + json.dumps({'invocationId': parent, 'invocationStatus': 'COMPLETE_INVOCATION_STATUS', 'success': True})
        replies = [subprocess.CompletedProcess([], 2, 'https://app.buildbuddy.io/invocation/' + parent + '\n' +
                   'still running: ' + parent + '; resume with: bbrun.py wait ' + parent + '\n'),
                   subprocess.CompletedProcess([], 0, verdict + '\n'),
                   subprocess.CompletedProcess([], 0, 'FAKE retained runner, no wrapper authority\n')]
        output = D['owned_directory'](self.root / 'fake-wait')
        with patch.object(subprocess, 'run', side_effect=replies) as mocked, \
                patch.dict(S['wait_own'].__globals__, {'collect_terminal': lambda *args: {'overall': 'UNKNOWN'}}):
            self.assertEqual(S['wait_own'](['fake-sentinel-no-Bazel'], output, self.config(), {}), 3)
        commands = [call.args[0] for call in mocked.call_args_list]
        self.assertEqual(commands[0], ['fake-sentinel-no-Bazel'])
        self.assertEqual(commands[1], [*S['BB'], 'wait', parent, '--max-wait', '600'])
        self.assertEqual(commands[2][3], parent)
        observation = D['loads']((output / 'terminal-attribution.json').read_bytes())
        self.assertEqual(observation['parent'], 'PASS')
        self.assertEqual(observation['overall'], 'UNKNOWN')
        hold = self.root / 'BB-HOLD'
        hold.touch()
        with patch.dict(S['wait_own'].__globals__, {'HOLD': hold}), patch.object(subprocess, 'run') as mocked:
            with self.assertRaisesRegex(ValueError, 'paused for BB hold'):
                S['wait_own'](['fake-sentinel-no-Bazel'], output, self.config(), {})
            mocked.assert_not_called()

    def test_review_wrapper_reasons(self):
        parent = '12345678-1234-1234-1234-123456789abc'
        for stage, values in (
                ('artifact_handoff', {'status': 'unknown', 'reason': 'handoff_not_observed'}),
                ('cleanup', {'status': 'unknown', 'targets_present': None, 'reason': 'cleanup_not_observed'}),
                ('scanner', {'status': 'fail', 'first_exit_code': 1, 'exit_code': 1,
                             'reason': 'receipt_scan_rejected'})):
            wrapper = fake_wrapper()
            wrapper[stage] = values
            wrapper['wrapper'] = {'original_exit_code': None, 'exit_code': None}
            checked = S['validate_wrapper'](wrapper, parent)
            self.assertEqual(checked[stage], values)

    def test_review_terminal_failure_and_unavailability(self):
        parent = '12345678-1234-1234-1234-123456789abc'
        empty = subprocess.CompletedProcess([], 0, '')
        with patch.object(subprocess, 'run', return_value=empty):
            result = S['collect_terminal'](parent, 'FAIL', D['owned_directory'](self.root / 'failed-no-namespace'),
                                           '', self.config(), {})
        self.assertEqual(result['overall'], 'FAIL')
        verdict = 'verdict: ' + json.dumps({'invocationId': parent,
                   'invocationStatus': 'COMPLETE_INVOCATION_STATUS', 'success': True})
        for tag, error, overall, code in [('unavailable', OSError('unavailable'), 'UNKNOWN', 3),
                                           ('invalid', ValueError('contradiction'), 'FAIL', 1)]:
            output = D['owned_directory'](self.root / ('review-' + tag))
            replies = [subprocess.CompletedProcess([], 0, 'https://app.buildbuddy.io/invocation/' + parent + '\n' + verdict),
                       subprocess.CompletedProcess([], 0, '')]
            with patch.object(subprocess, 'run', side_effect=replies), \
                    patch.dict(S['wait_own'].__globals__, {'collect_terminal': lambda *a, exc=error: (_ for _ in ()).throw(exc)}):
                self.assertEqual(S['wait_own'](['fake-sentinel'], output, self.config(), {}), code)
            self.assertEqual(D['loads']((output / 'terminal-attribution.json').read_bytes())['overall'], overall)

    def test_review_wait_refusal_and_malformed_verdict(self):
        parent = '12345678-1234-1234-1234-123456789abc'
        for tag, code, body in [('usage', 2, 'usage error'), ('malformed', 0, 'verdict: {bad JSON}')]:
            output = D['owned_directory'](self.root / ('review-wait-' + tag))
            replies = [subprocess.CompletedProcess([], code, 'https://app.buildbuddy.io/invocation/' + parent + '\n' + body),
                       subprocess.CompletedProcess([], 0, '')]
            with patch.object(subprocess, 'run', side_effect=replies) as mocked:
                self.assertEqual(S['wait_own'](['fake-sentinel'], output, self.config(), {}), 3)
            self.assertFalse(any('wait' in call.args[0] for call in mocked.call_args_list))
            self.assertEqual(D['loads']((output / 'terminal-attribution.json').read_bytes())['overall'], 'UNKNOWN')

    def test_review_wait_cap(self):
        parent = '12345678-1234-1234-1234-123456789abc'
        pending = ('https://app.buildbuddy.io/invocation/' + parent + '\n'
                   'still running: ' + parent + '; resume with: bbrun.py wait ' + parent + '\n')
        def remote(command, **kwargs):
            return subprocess.CompletedProcess(command, 0, '') if 'bb_logs.py' in command[2] else subprocess.CompletedProcess(command, 2, pending)
        output = D['owned_directory'](self.root / 'review-wait-cap')
        with patch.object(subprocess, 'run', side_effect=remote) as mocked:
            self.assertEqual(S['wait_own'](['fake-sentinel', 'no', 'Bazel'], output, self.config(), {}), 3)
        waits = [call.args[0] for call in mocked.call_args_list if 'wait' in call.args[0]]
        self.assertEqual(len(waits), 3)
        self.assertTrue(all(command[4] == parent for command in waits))
        self.assertEqual(D['loads']((output / 'terminal-attribution.json').read_bytes())['reason'], 'same_invocation_wait_limit')

    def test_review_exact_remote_preparation(self):
        # Execute the precise embedded Python, not only compile it. This stops
        # before Bazel; fixtures run only inside the admitted producer action.
        artifact_root = self.root / 'remote-dry-run'
        artifact_root.mkdir()
        config = self.config()
        command = S['submission'](config, self.evidence / 'remote.log')
        code_index = command.index('-c')
        code = command[code_index + 1]
        argv = ['python3', '-I', '-c', code, *command[code_index + 2:], '--dry-run', '--source', str(self.source)]
        env = {**os.environ, 'BUILDBUDDY_ARTIFACTS_DIRECTORY': str(artifact_root)}
        run = subprocess.run(argv, cwd=self.source, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        self.assertEqual(run.returncode, 0, run.stdout[-2000:])
        self.assertEqual((self.source / D['RUNNER']).read_bytes(), self.original)
        self.assertTrue((artifact_root / 'verus-measurement' / 'runner-preview.rs').is_file())
        inside = self.source / 'must-not-create'
        bad = subprocess.run(['python3', '-I', '-c', code, *command[code_index + 2:], '--dry-run',
                              '--source', str(self.source), '--output', str(inside)], cwd=self.source,
                             env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        self.assertNotEqual(bad.returncode, 0)
        self.assertFalse(inside.exists())

    def test_review_current_main_clone_selector(self):
        config = self.config(selections=[{'target': '//crates/kernel:verus_kernel_clones',
                                         'selector': 'kernel#*::clone', 'expected': 33}])
        prepared, _, _ = D['prepare'](self.source, config, D['owned_directory'](self.root / 'unsplit-clone-preparation'))
        self.assertEqual(prepared['preparation'], 'PASS')

    def test_review_collect_preserves_failed_children(self):
        source = self.root / 'collect-fixture'
        source.mkdir()
        logs = source / 'actual-testlogs'
        logs.mkdir()
        (source / 'bazel-testlogs').symlink_to(logs, target_is_directory=True)
        config = self.config(selections=[configuration()['selections'][0],
            {'target': '//crates/kernel:verus_dependencies_clone',
             'selector': 'kernel::dependencies#Dependency::clone', 'expected': 1}])
        for selection in config.selections:
            directory = logs / selection.target[2:].replace(':', '/')
            directory.mkdir(parents=True)
            (directory / 'test.log').write_bytes(b'FAKE failed verifier output\n')
            (directory / 'test.xml').write_bytes(b'<testsuite tests="1" failures="1" errors="0"><testcase><failure/></testcase></testsuite>')
        output = D['owned_directory'](self.root / 'collect-failures')
        result = D['collect'](source, config, output, 1)
        self.assertEqual(len(result['rows']), 2)
        self.assertEqual(result['child'], 'FAIL')
        self.assertEqual(result['resources'], 'UNKNOWN')
        for index in range(2):
            self.assertIn(b'<failure/>', (output / f'{index}-test.xml').read_bytes())

    def test_review_stage_tree(self):
        source = self.root / 'raw-tree'
        source.mkdir()
        child = source / 'nested'
        child.mkdir()
        (child / 'one.air').write_bytes(b'FAKE raw query')
        (child / 'one.air').chmod(0o444)
        child.chmod(0o555)
        output = D['owned_directory'](self.root / 'raw-stage')
        manifest = []
        try:
            D['stage_tree'](source, output, 'query-', manifest)
            self.assertEqual((output / 'query-nested__one.air').read_bytes(), b'FAKE raw query')
            self.assertEqual(stat.S_IMODE((output / 'query-nested__one.air').stat().st_mode), 0o600)
            self.assertEqual(stat.S_IMODE(child.stat().st_mode), 0o555)
        finally:
            child.chmod(0o700)
        (source / 'escape').symlink_to(child, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, 'symlink'):
            D['stage_tree'](source, output, 'another-', [])

    def test_terminal_cas_collector_fake_wrapper(self):
        parent = '12345678-1234-1234-1234-123456789abc'
        summary = {k: 'PASS' for k in ('preparation', 'child', 'resources', 'collection', 'restoration')}
        summary.update(command_artifact_namespace='verus-measurement', reserved_name_collision=False)
        config = self.config()
        row = config.selections[0]
        summary['rows'] = [D['asdict'](row)]
        summary['rows'][0]['rlimit_run'] = 999  # Untrusted summary must not supply metrics.
        summary['rlimit_run_sum'] = 999
        expected = D['loads']((self.evidence / 'normal-receipt.json').read_bytes())
        objects = {p.name: p.read_bytes() for p in (self.evidence / 'normal').iterdir()
                   if not p.name.endswith('pyc') and p.name != 'submission-argv.json'}
        objects.update({'measurement-summary.json': D['encoded'](summary),
            '0-test.log': b'FAKE selected child log\n',
            '0-test.xml': b'<testsuite tests="1" failures="0" errors="0"><testcase/></testsuite>',
            '0-inventory.json': D['encoded']({'proofs': {row.target: {'kind': 'verus', 'module': row.selector,
                'verified': 1, 'expected_obligations': 1, 'threads': 1, 'seconds': 0}}}),
            '0-measurement.json': D['encoded']({'label': row.target, 'verified': 1, 'seconds': 0,
                'rlimit_budget': 30, 'rlimit_run': 0, 'rlimit_init': 7, 'function_rlimit_sum': 0, 'function_count': 1, 'max_function_rlimit': 0}),
            '0-execution.json': D['encoded'](fake_execution(row)),
            '0-verifier.json': D['encoded']({'verification-results': {'verified': 1, 'errors': 0,
                'encountered-error': False, 'encountered-vir-error': False, 'is-verifying-entire-crate': False},
                'times-ms': {'total': 0, 'num-threads': 1, 'smt': {'rlimit-run': 0, 'rlimit-init': 7, 'smt-run-module-times': [{'function-breakdown': [{'rlimit': 0, 'success': True}]}]}}})})
        manifest = [{'name': name, 'sha256': D['digest'](data), 'bytes': len(data)} for name, data in objects.items()]
        files = {'verus-measurement/' + name: data for name, data in objects.items()}
        files.update({'verus-measurement/artifact-manifest.json': D['encoded'](manifest),
                      S['RESERVED']: D['encoded'](fake_wrapper())})
        def remote(command, **kwargs):
            self.assertEqual(command[:5], [*S['BB'], 'artifacts', parent])
            metadata = [{'name': name, 'uri': 'bytestream://FAKE/blobs/' + D['digest'](data) + '/' + str(len(data))}
                        for name, data in files.items()]
            if '--fetch' in command:
                name = command[command.index('--fetch') + 1]
                Path(command[command.index('--out') + 1]).write_bytes(files[name])
            return subprocess.CompletedProcess(command, 0, '\n'.join(json.dumps(m) for m in metadata))
        with patch.object(subprocess, 'run', side_effect=remote):
            result = S['collect_terminal'](parent, 'PASS', D['owned_directory'](self.root / 'fake-cas'), '', config, expected)
            self.assertEqual(result['overall'], 'PASS')
            self.assertEqual(result['driver']['rlimit_run_sum'], 0)
            self.assertEqual(result['driver']['rows'][0]['rlimit_run'], 0)
            self.assertTrue(all(a['parent_invocation_id'] == parent for a in result['cas']))
            self.assertEqual(len(result['cas']), 3 + len(manifest))
            files.pop(S['RESERVED'])
            result = S['collect_terminal'](parent, 'PASS', D['owned_directory'](self.root / 'missing-v1'), '', config, expected)
            self.assertEqual(result['overall'], 'UNKNOWN')
            # A complete parent log is not a proven final wrapper-step body.
            result = S['collect_terminal'](parent, 'PASS', D['owned_directory'](self.root / 'fake-line'),
                                          S['MARKER'] + json.dumps(fake_wrapper()), config, expected)
            self.assertEqual(result['overall'], 'UNKNOWN')
            # Expressly fake, separately attributed wrapper-step stdout body.
            result = S['collect_terminal'](parent, 'PASS', D['owned_directory'](self.root / 'fake-wrapper-body'),
                S['MARKER'] + json.dumps(fake_wrapper()) + '\nRemote run completed', config, expected,
                wrapper_step_stdout=S['MARKER'] + json.dumps(fake_wrapper()))
            self.assertEqual(result['overall'], 'PASS')
            with self.assertRaisesRegex(ValueError, 'unambiguous final'):
                S['collect_terminal'](parent, 'PASS', D['owned_directory'](self.root / 'fake-untrusted-body'),
                    '', config, expected, wrapper_step_stdout=S['MARKER'] + json.dumps(fake_wrapper()) + '\nfooter')
            summary['reserved_name_collision'] = True
            files['verus-measurement/measurement-summary.json'] = D['encoded'](summary)
            with self.assertRaisesRegex(ValueError, 'collision'):
                S['collect_terminal'](parent, 'FAIL', D['owned_directory'](self.root / 'collision'), '', config, expected)


def main():
    result = unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(Fixtures))
    root = Fixtures.evidence
    root.joinpath('fixture-result.json').write_text(json.dumps({'success': result.wasSuccessful(),
        'tests': result.testsRun, 'errors': len(result.errors), 'failures': len(result.failures),
        'exact_fixture_head': HEAD, 'fixture_shallow': True,
        'bazel_executed': False, 'verus_executed': False, 'real_scanner_executed': False}, indent=2))
    for file in root.rglob('*'):
        if file.is_file():
            file.chmod(0o600)  # Newly owned producer evidence only.
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode='w:gz') as archive:
        archive.add(root, arcname='evidence')
    blob = buffer.getvalue()
    print('FIXTURE_EVIDENCE_SHA256 ' + D['digest'](blob) + ' ' + str(len(blob)))
    print('FIXTURE_EVIDENCE_BASE64 ' + base64.b64encode(blob).decode())
    raise SystemExit(0 if result.wasSuccessful() else 1)


if __name__ == '__main__':
    main()
