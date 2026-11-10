"""Admitted-only producer fixtures. No Bazel, Verus or real scanner execution."""
import base64
import copy
import gzip
import io
import json
import os
from pathlib import Path
import runpy
import signal
import stat
import subprocess
import tarfile
import tempfile
import time
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

    def test_production_checking_preparation_and_argv(self):
        selection = {'target': '//crates/kernel:verus_checking',
                     'selector': 'kernel::checking#*World::check', 'expected': 10}
        for mode in ('frontend-only', 'normal'):
            config = self.config(mode=mode, selections=[selection])
            output = self.evidence / ('checking-' + mode)
            command = S['submission'](config, self.evidence / ('checking-' + mode + '.log'))
            receipt = self.evidence / ('checking-' + mode + '-receipt.json')
            prepared = S['full_dry_run'](self.source, config, output, receipt, command)
            self.assertEqual(prepared['full_preparation'], 'PASS')
            S['check_receipt'](self.source, config, receipt, command)
            self.assertEqual(config.command()[-1], selection['target'])
            index = command.index('-c')
            payload = S['unpack_transport'](command[index + 2], command[index + 3])
            self.assertEqual(D['loads'](payload['config.json'])['selections'], [selection])
            row = config.selections[0]
            execution = fake_execution(row, mode)
            D['validate_execution'](execution, row, mode)
            self.assertEqual(execution['argv'][execution['argv'].index('--verify-function') + 1], '*World::check')
        for selector in ('kernel::checking#**World::check', 'kernel::checking#World::*',
                         'kernel::*#World::check', 'kernel::checking#*', 'kernel::checking#World::check;echo'):
            with self.subTest(selector=selector), self.assertRaises(ValueError):
                self.config(selections=[{**selection, 'selector': selector}])
        for change in ({'selector': 'kernel::checking#*World::chek'}, {'expected': 11}):
            config = self.config(selections=[{**selection, **change}])
            with self.assertRaisesRegex(ValueError, 'selector/count differs from exact source'):
                D['prepare'](self.source, config, D['owned_directory'](self.root / ('checking-invalid-' + str(len(change.get('selector', ''))))))

    def test_all_342_production_selectors_parse(self):
        inventory = D['loads']((HERE / 'production-selectors.fixture.json').read_bytes())
        self.assertEqual(len(inventory['selections']), inventory['groups'])
        self.assertEqual(inventory['groups'], 541)
        self.assertEqual(sum(row['expected'] for row in inventory['selections']), inventory['obligations'])
        rows = []
        for selection in inventory['selections']:
            try:
                config = self.config(selections=[selection])
                self.assertEqual(D['asdict'](config.selections[0]), selection)
                rows.append({**selection, 'parse': 'PASS'})
            except ValueError as error:
                rows.append({**selection, 'parse': 'FAIL', 'reason': str(error)})
        self.evidence.joinpath('production-selector-audit.json').write_bytes(D['encoded']({
            **inventory, 'fixture_sha256': D['digest']((HERE / 'production-selectors.fixture.json').read_bytes()),
            'rows': rows, 'parse_passes': sum(row['parse'] == 'PASS' for row in rows)}))
        self.assertEqual([row for row in rows if row['parse'] != 'PASS'], [])
        self.config(selections=inventory['selections'])  # Preserve whole-manifest uniqueness checks.

    def production_selection_flags(self, selector):
        # Bind the audit's case split to the exact production command builder.
        source = self.original.decode()
        start = source.index("        let (selected_module, function) = module")
        end = source.index("        match mode {", start)
        block = source[start:end]
        expected = """        let (selected_module, function) = module
            .split_once('#')
            .map_or((module, None), |(owner, function)| (owner, Some(function)));
        if selected_module == "root" {
            process.arg("--verify-root");
        } else if !selected_module.is_empty() {
            process.args(["--verify-only-module", selected_module]);
        }
        if let Some(function) = function {
            process.args(["--verify-function", function]);
        }
"""
        self.assertEqual(block, expected, 'production selector flag convention changed; re-audit all kinds')
        owner, _, function = selector.partition('#')
        flags = ['--verify-root'] if owner == 'root' else ['--verify-only-module', owner]
        return flags + (['--verify-function', function] if function else [])

    def production_execution(self, row, mode):
        execution = fake_execution(row, mode)
        argv = execution['argv']
        begin = argv.index('--verify-only-module')
        del argv[begin:begin + 2]
        if '--verify-function' in argv:
            begin = argv.index('--verify-function')
            del argv[begin:begin + 2]
        argv += self.production_selection_flags(row.selector)
        return execution

    def test_production_root_preparation_and_execution(self):
        selection = {'target': '//crates/kernel:verus_command_headroom', 'selector': 'root', 'expected': 1}
        for mode in ('frontend-only', 'normal'):
            config = self.config(mode=mode, selections=[selection])
            output = self.evidence / ('root-' + mode)
            command = S['submission'](config, self.evidence / ('root-' + mode + '.log'))
            receipt = self.evidence / ('root-' + mode + '-receipt.json')
            prepared = S['full_dry_run'](self.source, config, output, receipt, command)
            self.assertEqual(prepared['full_preparation'], 'PASS')
            S['check_receipt'](self.source, config, receipt, command)
            self.assertEqual(config.command()[-1], selection['target'])
            execution = self.production_execution(config.selections[0], mode)
            self.assertIn('--verify-root', execution['argv'])
            self.assertNotIn('--verify-only-module', execution['argv'])
            D['validate_execution'](execution, config.selections[0], mode)
            for flags in ([], ['--verify-only-module', 'root'], ['--verify-root', '--verify-only-module', 'root'], ['--verify-root', '--verify-root']):
                bad = copy.deepcopy(execution)
                bad['argv'].remove('--verify-root')
                bad['argv'] += flags
                with self.subTest(mode=mode, flags=flags), self.assertRaises(ValueError):
                    D['validate_execution'](bad, config.selections[0], mode)
        row = self.config().selections[0]
        for flags in (['--verify-root'], ['--verify-root', '--verify-only-module', 'kernel::dependencies'], ['--verify-only-module', 'root']):
            bad = self.production_execution(row, 'normal')
            begin = bad['argv'].index('--verify-only-module')
            del bad['argv'][begin:begin + 2]
            bad['argv'] += flags
            with self.subTest(flags=flags), self.assertRaises(ValueError):
                D['validate_execution'](bad, row, 'normal')

    def test_all_342_production_selection_flags(self):
        inventory = D['loads']((HERE / 'production-selectors.fixture.json').read_bytes())
        rows = []
        for selection in inventory['selections']:
            row = self.config(selections=[selection]).selections[0]
            kind = 'root' if row.selector == 'root' else 'function' if '#' in row.selector else 'module'
            try:
                execution = self.production_execution(row, 'frontend-only')
                D['validate_execution'](execution, row, 'frontend-only')
                rows.append({**selection, 'kind': kind, 'flags': self.production_selection_flags(row.selector), 'validation': 'PASS'})
            except ValueError as error:
                rows.append({**selection, 'kind': kind, 'validation': 'FAIL', 'reason': str(error)})
        self.evidence.joinpath('production-flags-audit.json').write_bytes(D['encoded']({
            'source_head': inventory['source_head'], 'runner_sha256': D['digest'](self.original),
            'production_builder_source_guard': True, 'executions': 'synthetic, not Verus runs',
            'rows': rows, 'validation_passes': sum(row['validation'] == 'PASS' for row in rows)}))
        self.assertEqual(len(rows), 541)
        self.assertEqual({row['kind'] for row in rows}, {'root', 'module', 'function'})
        self.assertEqual([row for row in rows if row['validation'] != 'PASS'], [])

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
        for key in ('rlimit_run', 'function_rlimit_sum', 'function_count', 'max_function_rlimit', 'seconds'):
            bad = {**measurement, key: False if key != 'function_count' else True}
            with self.subTest(key=key), self.assertRaises(ValueError):
                D['child_verdict'](stats, proof, bad, row, execution=fake_execution(row))
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
                 'times-ms': {'total': 1, 'num-threads': 1, 'smt': {**{k: 0 for k in ('total', 'smt-init', 'smt-run', 'rlimit-init', 'rlimit-run')},
                     'smt-init-module-times': [], 'smt-run-module-times': []}}}
        proof = {'proofs': {'@@' + row.target: {'kind': 'FRONTEND_ONLY', 'module': row.selector, 'verified': 0,
                 'expected_obligations': row.expected, 'threads': 1, 'seconds': 1}}}
        metrics = {'label': '@@' + row.target, 'mode': 'FRONTEND_ONLY', 'no_smt': True,
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

    def test_bazel_main_repository_label_and_required_downloads(self):
        row = self.config().selections[0]
        for label in (row.target, '@@' + row.target):
            self.assertEqual(D['inventory_label']({'proofs': {label: {}}}, row), label)
        for label in ('@external' + row.target, '@' + row.target, '@@@' + row.target, '//wrong:target'):
            with self.assertRaises(ValueError):
                D['inventory_label']({'proofs': {label: {}}}, row)
        regex = next(arg.split('=', 1)[1] for arg in self.config().command() if arg.startswith('--remote_download_regex='))
        import re
        for name in ('/testlogs/test.xml', '/testlogs/test.log', '/testlogs/test.outputs/outputs.zip'):
            self.assertIsNotNone(re.fullmatch(regex, name))
        self.assertIsNone(re.fullmatch(regex, '/execroot/unused_object.o'))

    def test_frontend_ticket_and_stale_mode_source_argv(self):
        config = self.config()
        output = D['owned_directory'](self.root / 'ticket-preparation')
        prepared, _, _ = D['prepare'](self.source, config, output)
        ticket = {'schema': 'tl3.verus-measurement.frontend-prerequisite.v1',
                  'parent_invocation_id': '12345678-1234-1234-1234-123456789abc', 'no_smt': True,
                  **{key: prepared[key] for key in ('source_sha256', 'selection_sha256', 'runner_sha256', 'pins_sha256')},
                  'driver_sha256': D['tooling_identity']()['driver_sha256'],
                  'frontend_bazel_argv': self.config(mode='frontend-only').command(),
                  'executions': [fake_execution(config.selections[0], 'frontend-only')]}
        D['validate_frontend_ticket'](ticket, config, prepared)
        for key, value in [('source_sha256', 'b' * 64), ('runner_sha256', 'b' * 64),
                           ('driver_sha256', 'b' * 64), ('no_smt', False),
                           ('frontend_bazel_argv', config.command()), ('executions', [])]:
            with self.subTest(key=key), self.assertRaises(ValueError):
                D['validate_frontend_ticket']({**ticket, key: value}, config, prepared)
        with self.assertRaises(ValueError):
            D['validate_frontend_ticket'](None, config, prepared)
        receipt = self.evidence / 'ticket-dry-receipt.json'
        S['full_dry_run'](self.source, config, self.evidence / 'ticket-dry', receipt, S['submission'](config, self.evidence / 'runner.log'))
        with self.assertRaisesRegex(ValueError, 'stale/mismatched'):
            S['check_receipt'](self.source, self.config(mode='frontend-only'), receipt,
                              S['submission'](self.config(mode='frontend-only'), self.evidence / 'runner.log'))

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

    def assert_unidentified_submission(self, tag, transcript, code, candidates):
        output = D['owned_directory'](self.root / tag)
        hold = self.root / ('absent-' + tag)
        config = self.config()
        reply = subprocess.CompletedProcess([], code, transcript)
        with patch.dict(S['wait_own'].__globals__, {'HOLD': hold}), \
                patch.object(subprocess, 'run', return_value=reply) as mocked:
            with self.assertRaisesRegex(ValueError, 'submission lacks unique own invocation'):
                S['wait_own'](['FAKE-no-execution'], output, config, {'fixture': True})
        self.assertEqual(mocked.call_count, 1)  # No replay, wait or API inspection.
        self.assertTrue((output / 'submission-client.txt').exists(), 'lost client response before own-ID guard')
        self.assertEqual((output / 'submission-client.txt').read_bytes(), transcript.encode() if isinstance(transcript, str) else transcript)
        self.assertEqual(D['loads']((output / 'submission-client-status.json').read_bytes()), {'client_exit': code})
        result = D['loads']((output / 'terminal-attribution.json').read_bytes())
        self.assertEqual(result['overall'], 'UNKNOWN')
        self.assertEqual(result['parent'], 'UNKNOWN')
        self.assertIsNone(result['parent_invocation_id'])
        self.assertEqual(result['client_exit'], code)
        self.assertEqual(result['invocation_candidates'], candidates)
        self.assertEqual(result['config'], config.json())
        self.assertEqual(result['preparation_identity'], {'fixture': True})

    def test_submission_missing_id_retains_evidence(self):
        self.assert_unidentified_submission('no-id', 'FAKE response without ID\n', 0, [])

    def test_submission_ambiguous_ids_retains_evidence(self):
        ids = ['12345678-1234-1234-1234-123456789abc', '22345678-1234-1234-1234-123456789abc']
        text = ''.join('https://app.buildbuddy.io/invocation/' + id + '\n' for id in ids)
        self.assert_unidentified_submission('ambiguous-ids', text, 0, ids)

    def test_submission_nonzero_exit_retains_evidence(self):
        self.assert_unidentified_submission('no-id-error', b'FAKE client refused before Run\r\n\xff', 2, [])

    def test_greedy_ordered_batches(self):
        rows = D['loads']((HERE / 'production-selectors.fixture.json').read_bytes())['selections'][:5]
        config = self.config(mode='frontend-only', selections=rows)

        def plan_for(current, fits, estimate=None):
            def size(command):
                index = command.index('-c')
                decoded = S['unpack_transport'](command[index + 2], command[index + 3])
                selected = D['loads'](decoded['config.json'])['selections']
                start, end = rows.index(selected[0]), rows.index(selected[-1]) + 1
                accepted = fits(start, end)
                return {'fits': accepted, 'estimated_serialized_bytes': estimate or (100 if accepted else 5000),
                        'limit_bytes': 4096}
            with patch.dict(S['batch_plan'].__globals__, {'stock_size': size}):
                return S['batch_plan'](current, self.evidence / 'greedy.log', None)

        intervals = lambda plan: [(item['start'], item['end']) for item in plan]
        # Stop at the first overflow even when a later gzip size could fit again.
        allowed = {(0, 1), (0, 2), (1, 4), (2, 3), (3, 4)}
        four = self.config(mode='frontend-only', selections=rows[:4])
        self.assertEqual(intervals(plan_for(four, lambda i, j: (i, j) in allowed)), [(0, 2), (2, 3), (3, 4)])
        first = plan_for(config, lambda i, j: j - i <= 2)
        self.assertEqual(intervals(first), [(0, 2), (2, 4), (4, 5)])
        self.assertEqual(first, plan_for(config, lambda i, j: j - i <= 2))
        self.assertEqual(intervals(plan_for(config, lambda i, j: True, estimate=3000)),
                         [(i, i + 1) for i in range(5)])
        with self.assertRaisesRegex(ValueError, 'single selector exceeds stock size limit: ' + rows[0]['target']):
            plan_for(config, lambda i, j: False)

    def test_342_selector_planning_bound(self):
        # Synthetic inventory, real stock serializer, no network or Run.
        rows = []
        for index in range(342):
            name = D['digest'](str(index).encode())
            rows.append({'target': '//crates/kernel:verus_synthetic_' + name,
                         'selector': 'kernel::synthetic#proof_' + name, 'expected': 1})
        config = self.config(mode='frontend-only', selections=rows)
        stock_size = S['stock_size']
        runs, plans = [], []

        def timed_out(signum, frame):
            raise TimeoutError('342-selector planner exceeded 30-second fixture bound')

        previous = signal.signal(signal.SIGALRM, timed_out)
        try:
            for repeat in range(2):
                calls = 0
                started = time.monotonic()
                complete = False

                def counted(command):
                    nonlocal calls
                    calls += 1
                    return stock_size(command)

                signal.setitimer(signal.ITIMER_REAL, 30)
                try:
                    with patch.dict(S['batch_plan'].__globals__, {'stock_size': counted}):
                        plan = S['batch_plan'](config, self.evidence / 'scale.log', None)
                    complete = True
                finally:
                    signal.setitimer(signal.ITIMER_REAL, 0)
                    runs.append({'repeat': repeat, 'complete': complete, 'seconds': time.monotonic() - started,
                                 'stock_serializations': calls, 'bound_seconds': 30})
                    self.evidence.joinpath('planner-scale.json').write_bytes(D['encoded']({
                        'inventory': '342 deterministic synthetic selectors; no proof execution', 'runs': runs}))
                self.assertLessEqual(calls, 3 * len(rows))
                self.assertEqual([row for item in plan for row in item['config']['selections']], rows)
                self.assertTrue(all(item['size']['fits'] for item in plan))
                for item in plan:
                    self.assertEqual(stock_size(item['command']), item['size'])
                plans.append(plan)
            self.assertEqual(plans[0], plans[1])
            self.evidence.joinpath('planner-scale.json').write_bytes(D['encoded']({
                'inventory': '342 deterministic synthetic selectors; no proof execution',
                'runs': runs, 'deterministic': True, 'all_final_stock_guards_pass': True,
                'batches': [{'start': item['start'], 'end': item['end'], 'size': item['size']} for item in plans[0]]}))
        finally:
            signal.setitimer(signal.ITIMER_REAL, 0)
            signal.signal(signal.SIGALRM, previous)

    def pair_config(self, mode='normal'):
        first = configuration()['selections'][0]
        second = {'target': '//crates/kernel:verus_kernel_credit_balance', 'selector': 'kernel#Credit::balance', 'expected': 1}
        return self.config(mode=mode, selections=[first, second])

    def test_projected_frontend_ticket(self):
        config = self.pair_config()
        frontend = self.pair_config('frontend-only')
        expected = {k: 'a' * 64 for k in ('source_sha256', 'selection_sha256', 'runner_sha256', 'pins_sha256')}
        expected['driver_sha256'] = D['tooling_identity']()['driver_sha256']
        prior = {'overall': 'PASS', 'parent_invocation_id': '12345678-1234-1234-1234-123456789abc',
                 'config': frontend.json(), 'preparation_identity': expected,
                 'driver': {'executions': [fake_execution(row, 'frontend-only') for row in frontend.selections]}}
        current = S['subset'](config, 1, 2)
        ticket = S['frontend_ticket'](prior, current)
        prepared = {**expected, 'selection_sha256': D['digest'](D['encoded']([D['asdict'](current.selections[0])]))}
        self.assertEqual(D['validate_frontend_ticket'](ticket, current, prepared), ticket)
        self.assertEqual(ticket['parents'][0]['config'], frontend.json())
        self.assertEqual(ticket['parents'][0]['indices'], [1])
        for change in ('source', 'indices', 'mode', 'duplicate', 'execution'):
            bad = copy.deepcopy(ticket)
            if change == 'source': bad['source_sha256'] = 'b' * 64
            if change == 'indices': bad['parents'][0]['indices'] = [0]
            if change == 'mode': bad['parents'][0]['config']['mode'] = 'normal'
            if change == 'duplicate': bad['parents'] *= 2
            if change == 'execution': bad['executions'][0]['argv'] += ['--build-sst']
            with self.subTest(change=change), self.assertRaises(ValueError):
                D['validate_frontend_ticket'](bad, current, prepared)

    def test_batch_preparation_execution_and_ids(self):
        config = self.pair_config('frontend-only')
        def size(command):
            index = command.index('-c')
            decoded = S['unpack_transport'](command[index + 2], command[index + 3])
            count = len(D['loads'](decoded['config.json'])['selections'])
            return {'fits': count == 1, 'estimated_serialized_bytes': count * 10, 'limit_bytes': 10}
        def completed(command, output, child_config, expected):
            index = config.json()['selections'].index(child_config.json()['selections'][0])
            parent = str(index + 1) + '2345678-1234-1234-1234-123456789abc'
            observation = {'overall': 'PASS', 'parent': 'PASS', 'parent_invocation_id': parent,
                           'config': child_config.json(), 'cas': [], 'preparation_identity': expected}
            D['put'](output, 'terminal-attribution.json', D['encoded'](observation))
            return 0
        with patch.dict(S['batch_plan'].__globals__, {'stock_size': size}):
            plan = S['batch_plan'](config, self.evidence / 'batch.log', None)
            self.assertEqual([(i['start'], i['end']) for i in plan], [(0, 1), (1, 2)])
            receipt = self.evidence / 'batch-root-receipt.json'
            S['prepare_batches'](self.source, config, self.root / 'batch-preparation', receipt, plan)
            output = self.root / 'batch-run'
            with patch.dict(S['run_batches'].__globals__, {'wait_own': completed}):
                self.assertEqual(S['run_batches'](self.source, config, output, receipt, plan, None), 0)
            result = D['loads']((output / 'terminal-attribution.json').read_bytes())
            self.assertEqual(result['overall'], 'PASS')
            self.assertEqual(len(set(result['parent_invocation_ids'])), 2)
            records = result['records']
            bad = copy.deepcopy(records)
            bad[1]['parent_invocation_id'] = bad[0]['parent_invocation_id']
            with self.assertRaisesRegex(ValueError, 'duplicate batch own ID'):
                S['merge_batches'](config, plan, bad)
            bad = copy.deepcopy(records)
            bad[1]['cas'] = [{'parent_invocation_id': bad[0]['parent_invocation_id']}]
            with self.assertRaisesRegex(ValueError, 'batch CAS own ID mismatch'):
                S['merge_batches'](config, plan, bad)
            self.assertEqual(S['merge_batches'](config, plan, records[:1])['overall'], 'UNKNOWN')
            with self.assertRaisesRegex(ValueError, 'batch config/coverage'):
                S['merge_batches'](config, plan, records[::-1])
            def incomplete(command, output, child_config, expected):
                D['put'](output, 'terminal-attribution.json', D['encoded']({
                    'overall': 'UNKNOWN', 'parent': 'UNKNOWN', 'parent_invocation_id': None,
                    'config': child_config.json()}))
                return 3
            with patch.dict(S['run_batches'].__globals__, {'wait_own': incomplete}):
                stopped = self.root / 'batch-stop'
                self.assertEqual(S['run_batches'](self.source, config, stopped, receipt, plan, None), 3)
            self.assertTrue((stopped / 'batch-0000').exists())
            self.assertFalse((stopped / 'batch-0001').exists())
            self.assertEqual(D['loads']((stopped / 'terminal-attribution.json').read_bytes())['overall'], 'UNKNOWN')
            stale = copy.deepcopy(plan)
            stale[0]['command'] += ['changed']
            with self.assertRaisesRegex(ValueError, 'stale batch'):
                S['run_batches'](self.source, config, self.root / 'batch-stale', receipt, stale, None)

    def test_batched_frontend_audits_each_own_record(self):
        config = self.pair_config()
        frontend = self.pair_config('frontend-only')
        records = [{'overall': 'PASS', 'parent_invocation_id': str(i + 1) + '2345678-1234-1234-1234-123456789abc',
                    'config': S['subset'](frontend, i, i + 1).json()} for i in range(2)]
        prior = {'schema': 'tl3.verus-measurement.batches.v1', 'overall': 'PASS',
                 'config': frontend.json(), 'records': records}
        path = self.evidence / 'batched-frontend-locator.json'
        path.write_bytes(D['encoded'](prior))
        calls = []
        def audit(source, cfg, locator, output):
            calls.append(D['loads'](locator.read_bytes()))
            return {'driver': 'FAKE independently reaudited fields'}
        with patch.dict(S['check_frontend'].__globals__, {'check_frontend_record': audit}):
            result = S['check_frontend'](self.source, config, path, self.evidence)
        self.assertEqual(result, prior)
        self.assertEqual(calls, records)
        for change in ('coverage', 'duplicate-id', 'failure'):
            bad = copy.deepcopy(prior)
            if change == 'coverage': bad['records'].reverse()
            if change == 'duplicate-id': bad['records'][1]['parent_invocation_id'] = bad['records'][0]['parent_invocation_id']
            if change == 'failure': bad['records'][0]['overall'] = 'FAIL'
            path.write_bytes(D['encoded'](bad))
            with patch.dict(S['check_frontend'].__globals__, {'check_frontend_record': audit}), self.assertRaises(ValueError):
                S['check_frontend'](self.source, config, path, self.evidence)

    def test_compact_saved_six_helper_transport(self):
        saved = D['loads'](gzip.decompress((HERE / 'six-helper-argv.fixture.json.gz').read_bytes()))
        stock = runpy.run_path(S['BB'][-1], run_name='fixture_stock_no_main')
        old_body = stock['run_request'](stock['parse_args'](saved[3:]))
        with self.assertRaises(stock['ActionTooLarge']):
            stock['check_action_size'](json.dumps(old_body).encode())
        config = D['Config'].parse(D['loads'](saved[saved.index('--config-json') + 1]))
        ticket = D['loads'](saved[saved.index('--frontend-prerequisite-json') + 1])
        # The historical caba head is outside the shallow fixture Git.
        with patch.dict(S['transport_payload'].__globals__, {'pinned_driver': lambda *_: (HERE / 'measure-remote.py').read_bytes()}):
            command = S['submission'](config, self.evidence / 'six-helper.log', ticket)
        new_body = stock['run_request'](stock['parse_args'](command[3:]))
        stock['check_action_size'](json.dumps(new_body).encode())
        self.evidence.joinpath('six-helper-size.json').write_bytes(D['encoded']({
            'old': S['stock_size'](saved), 'compact': S['stock_size'](command)}))
        with patch.dict(S['transport_payload'].__globals__, {'pinned_driver': lambda *_: (HERE / 'measure-remote.py').read_bytes()}):
            payload, identity = S['transport_payload'](config, ticket)
            self.assertEqual(S['transport_payload'](config, ticket), (payload, identity))
        decoded = S['unpack_transport'](payload, identity)
        self.assertEqual(decoded['measure-remote.py'], (HERE / 'measure-remote.py').read_bytes())
        self.assertEqual(decoded['config.json'], D['encoded'](config.json()))
        self.assertEqual(decoded['frontend.json'], D['encoded'](ticket))
        with self.assertRaises(ValueError):
            S['unpack_transport'](payload, '0' * 64)
        records = D['loads'](gzip.decompress(base64.b64decode(payload)))
        records['config.json']['bytes'] += 1
        corrupt = gzip.compress(D['encoded'](records), mtime=0)
        with self.assertRaisesRegex(ValueError, 'hash/length mismatch'):
            S['unpack_transport'](base64.b64encode(corrupt).decode(), D['digest'](corrupt))

    def test_checkout_driver_transport(self):
        config = self.config()
        ticket = {'FAKE': 'byte-preserved frontend ticket'}
        payload, identity = S['transport_payload'](config, ticket, self.source)
        records = D['loads'](gzip.decompress(base64.b64decode(payload)))
        driver = records['measure-remote.py']
        self.assertEqual(set(driver), {'path', 'bytes', 'sha256'})
        self.assertEqual(driver['path'], 'tools/ci/verus_measurement/measure-remote.py')
        decoded = S['unpack_transport'](payload, identity, self.source)
        self.assertEqual(decoded['measure-remote.py'], (HERE / 'measure-remote.py').read_bytes())
        self.assertEqual(decoded['config.json'], D['encoded'](config.json()))
        self.assertEqual(decoded['frontend.json'], D['encoded'](ticket))
        missing = self.root / 'missing-driver-checkout'
        missing.mkdir()
        with self.assertRaisesRegex(ValueError, 'missing/nonregular checkout driver'):
            S['unpack_transport'](payload, identity, missing)
        with self.assertRaises(subprocess.CalledProcessError):
            S['transport_payload'](self.config(head='f' * 40), ticket, self.source)

    def test_checkout_bootstrap_and_embedded_fallback(self):
        # Fake driver only: executing it records its copied bytes and identity.
        driver_bytes = b'''import hashlib,json,os\nfrom pathlib import Path\np=Path(__file__)\nPath(os.environ["BUILDBUDDY_ARTIFACTS_DIRECTORY"],"observed.json").write_text(json.dumps({"sha256":hashlib.sha256(p.read_bytes()).hexdigest(),"identity":__driver_sha256__,"file":str(p),"mode":p.stat().st_mode & 0o777,"argv":__import__("sys").argv}))\n'''
        driver = self.root / 'fake-measure-remote.py'
        driver.write_bytes(driver_bytes)
        cwd = self.root / 'bootstrap-source'
        target = cwd / 'tools/ci/verus_measurement/measure-remote.py'
        target.parent.mkdir(parents=True)
        output = self.root / 'bootstrap-output'
        output.mkdir()
        marker = output / 'observed.json'
        config = self.config()
        with patch.dict(S['transport_payload'].__globals__, {'DRIVER': driver, 'pinned_driver': lambda *_: driver_bytes}):
            command = S['submission'](config, self.evidence / 'bootstrap.log', {'FAKE': True}, self.source)
        args = command[command.index('--') + 1:]
        env = {**os.environ, 'BUILDBUDDY_ARTIFACTS_DIRECTORY': str(output)}
        results = {}
        for name, content in [('missing', None), ('mismatch', b'x' * len(driver_bytes)), ('equal', driver_bytes)]:
            if content is not None:
                target.write_bytes(content)
            run = subprocess.run(args, cwd=cwd, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
            results[name] = {'status': run.returncode, 'stdout': run.stdout, 'executed': marker.exists()}
            if name != 'equal':
                self.assertNotEqual(run.returncode, 0)
                self.assertFalse(marker.exists(), 'driver executed before identity verification')
                self.assertIn('missing/nonregular' if name == 'missing' else 'hash/length mismatch', run.stdout)
            else:
                self.assertEqual(run.returncode, 0, run.stdout)
                observed = D['loads'](marker.read_bytes())
                self.assertEqual(observed['sha256'], D['digest'](driver_bytes))
                self.assertEqual(observed['identity'], D['digest'](driver_bytes))
                self.assertEqual(observed['mode'], 0o600)
                self.assertIn('--frontend-prerequisite-json', observed['argv'])
                self.assertEqual(Path(observed['file']).name, 'measure-remote.py')
                self.assertFalse(Path(observed['file']).exists())
                marker.unlink()
        # Explicit driver/source divergence uses the existing embedded path.
        with patch.dict(S['transport_payload'].__globals__, {'DRIVER': driver}):
            command = S['submission'](config, self.evidence / 'fallback.log', {'FAKE': True}, self.source)
        index = command.index('-c')
        records = D['loads'](gzip.decompress(base64.b64decode(command[index + 2])))
        self.assertEqual(set(records['measure-remote.py']), {'bytes', 'sha256', 'data'})
        target.unlink()
        run = subprocess.run(command[command.index('--') + 1:], cwd=cwd, env=env,
                             text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        self.assertEqual(run.returncode, 0, run.stdout)
        self.assertEqual(D['loads'](marker.read_bytes())['sha256'], D['digest'](driver_bytes))
        results['embedded_fallback'] = {'status': run.returncode, 'executed': True}
        self.evidence.joinpath('checkout-bootstrap.json').write_bytes(D['encoded'](results))

    def test_genuine_clone_singleton_size(self):
        historical = D['loads'](gzip.decompress((HERE / 'clone-singleton-argv.fixture.json.gz').read_bytes()))
        self.assertEqual(historical['schema'], 'tl3.measurement.historical-singleton-input.v1')
        retained = base64.b64decode(historical['argv_gzip_base64'], validate=True)
        self.assertEqual(D['digest'](retained), '27e7a555e71a0597c4ac15dd2e6391224c44c5cfe9f897da91de1e146ced9dda')
        saved = D['loads'](gzip.decompress(retained))
        self.assertEqual(D['digest'](D['encoded'](saved)), '488c6377a1c2ad68c1a872cc6a58d15c95559ef3c6ad251354e45a6a7d73829e')
        old = S['stock_size'](saved)
        self.assertEqual(historical['historical_source'], 'e935630d61367d0f8438b90be70db63870fe7b0c')
        self.assertEqual(historical['argv_sha256'], D['digest'](D['encoded'](saved)))
        self.assertEqual(historical['client_sha256'], '89adf0520f367f7f14e3081a0bbd68ef71075fe03b2ad5ddf88c3fc237985f96')
        self.assertEqual(historical['scanner_sha256'], 'decd9ce671299ece1b245811af459349ed9932ee9b81b2c4c7a13cf96b94f609')
        historical_request = base64.b64decode(historical['request_json_base64'], validate=True)
        self.assertEqual(D['digest'](historical_request), historical['request_json_sha256'])
        request = D['loads'](historical_request)
        self.assertEqual(request['commit_sha'], saved[saved.index('--commit') + 1])
        self.assertEqual(request['branch'], saved[saved.index('--branch') + 1])
        historical_size = 4 * ((len(historical_request) + 8192 + 2) // 3)
        self.assertEqual(historical_size, 127680)
        self.assertEqual(old['limit_bytes'], 122880)
        self.assertGreater(historical_size, old['limit_bytes'])
        self.assertFalse(old['fits'])
        index = saved.index('-c')
        decoded = S['unpack_transport'](saved[index + 2], saved[index + 3])
        self.assertEqual(decoded['measure-remote.py'], (HERE / 'measure-remote.py').read_bytes())
        config = D['Config'].parse(D['loads'](decoded['config.json']))
        self.assertEqual(D['asdict'](config.selections[0]), {'target': '//crates/kernel:verus_kernel_clones',
            'selector': 'kernel#*::clone', 'expected': 33})
        # The historical head is outside the shallow fixture Git. Live pinned
        # tree lookup is covered separately without this stub.
        with patch.dict(S['transport_payload'].__globals__, {'pinned_driver': lambda *_: decoded['measure-remote.py']}):
            command = S['submission'](config, Path(saved[saved.index('--log-file') + 1]), D['loads'](decoded['frontend.json']))
        size = S['stock_size'](command)
        self.assertTrue(size['fits'])
        self.assertLessEqual(size['estimated_serialized_bytes'], size['limit_bytes'] - S['BATCH_SAFETY_MARGIN'])
        index = command.index('-c')
        self.assertEqual(S['unpack_transport'](command[index + 2], command[index + 3], self.source), decoded)
        historical_identity = {key: value for key, value in historical.items() if not key.endswith('_base64')}
        historical_identity.update(request_json_bytes=len(historical_request), estimated_serialized_bytes=historical_size)
        self.evidence.joinpath('clone-singleton-size.json').write_bytes(D['encoded'](
            {'historical': historical_identity, 'red': old, 'green': size}))

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

    def test_pinned_wrapper_source_echo_is_not_an_emitted_result(self):
        source = "            print('BBRUN_WRAPPER_RESULT_V1 ' + json.dumps(result(), sort_keys=True, separators=(',', ':')), flush=True)"
        echo = source.replace("'", "'\"'\"'")
        marker = S['MARKER'] + json.dumps(fake_wrapper())
        self.assertEqual(S['wrapper_log_lines'](echo + '\n' + marker), ([marker], 1))
        for text in [marker + '\n' + marker, 'embedded ' + marker, echo + '\n' + echo, echo + 'changed']:
            with self.assertRaises(ValueError):
                S['wrapper_log_lines'](text)

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
        ticket = {'schema': 'tl3.verus-measurement.frontend-prerequisite.v1',
                  'parent_invocation_id': parent, 'no_smt': True,
                  **{key: expected[key] for key in ('source_sha256', 'selection_sha256', 'runner_sha256', 'pins_sha256', 'driver_sha256')},
                  'frontend_bazel_argv': self.config(mode='frontend-only').command(),
                  'executions': [fake_execution(row, 'frontend-only')]}
        expected['exact_submission_argv'] = S['submission'](config, self.evidence / 'fake.log', ticket)
        objects = {p.name: p.read_bytes() for p in (self.evidence / 'normal').iterdir()
                   if not p.name.endswith('pyc') and p.name != 'submission-argv.json'}
        objects.update({'frontend-prerequisite.json': D['encoded'](ticket), 'measurement-summary.json': D['encoded'](summary),
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
