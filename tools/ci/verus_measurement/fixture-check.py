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
HEAD = '6c6efd421c7026b9277c6af3ebb27c74030219fe'


def configuration():
    return {'head': HEAD, 'comparison_base': '1' * 40, 'branch': 'fixture/measurement',
            'mode': 'normal', 'watchdog_seconds': 45, 'outer_seconds': 180,
            'coordinator_minutes': 30, 'diagnostic_authorization': None, 'raw_queries': False,
            'selections': [{'target': '//crates/kernel:verus_dependencies',
                            'selector': 'kernel::dependencies#World::record_dependency', 'expected': 1}]}


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
                        'https://github.com/RobTand/trustladderiii.git', HEAD], check=True)
        subprocess.run(['git', '-C', str(cls.source), 'checkout', '-q', '--detach', 'FETCH_HEAD'], check=True)
        assert D['git'](cls.source, 'rev-parse', '--is-shallow-repository').strip() == b'true'
        cls.original = (cls.source / D['RUNNER']).read_bytes()

    def config(self, **changes):
        return D['Config'].parse({**configuration(), **changes})

    def test_full_preparation_and_receipts(self):
        for mode in ('normal', 'diagnostic'):
            config = self.config(mode=mode, watchdog_seconds=45 if mode == 'normal' else 120,
                                 diagnostic_authorization=None if mode == 'normal' else 'FAKE fixture authorization')
            receipt = self.evidence / f'{mode}-receipt.json'
            command = S['submission'](config, self.evidence / 'runner.log')
            result = S['full_dry_run'](self.source, config, self.evidence / mode, receipt, command)
            S['check_receipt'](self.source, config, receipt, command)
            self.assertEqual(result['full_preparation'], 'PASS')
            preview = (self.evidence / mode / 'runner-preview.rs').read_text()
            self.assertIn(f'"{config.watchdog_seconds}s"', preview)
            self.assertIn('let measured = true;', preview)
            self.assertNotIn('--no-verify', preview)
            self.assertNotIn('--log-all', preview)
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
        anchors = ['"--num-threads",', '"--rlimit",', '.env("OMP_NUM_THREADS", "1")',
                   '.env("RAYON_NUM_THREADS", "1")', '.env("VERUS_Z3_PATH", &self.z3)',
                   'process.args(["--time", "--output-json"]);',
                   '.args(["--kill-after=5s", "45s"])',
                   'verdict["success"].as_bool() != Some(true)',
                   '    let result = toolchain.verify(source, module, measured)?;',
                   '"{label}: group exceeds the hard 20s ceiling ({seconds:.3}s)"']
        text = self.original.decode()
        for anchor in anchors:
            with self.subTest(anchor=anchor), self.assertRaises(ValueError):
                D['instrument'](text.replace(anchor, 'MISSING_ANCHOR'), self.config())
        changed = D['instrument'](text, self.config()).encode()
        with self.assertRaisesRegex(RuntimeError, 'fixture exception'):
            with D['disposable_runner'](self.source, self.original, changed):
                self.assertEqual((self.source / D['RUNNER']).read_bytes(), changed)
                raise RuntimeError('fixture exception')
        self.assertEqual((self.source / D['RUNNER']).read_bytes(), self.original)
        self.assertIn('--log-all', D['instrument'](text, self.config(raw_queries=True)))

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
                 'times-ms': {'total': 0, 'num-threads': 1, 'smt': {'rlimit-run': 0, 'rlimit-init': 0}}}
        proof = {'proofs': {row.target: {'kind': 'verus', 'module': row.selector, 'verified': 1,
                  'expected_obligations': 1, 'threads': 1, 'seconds': 0}}}
        measurement = {'label': row.target, 'verified': 1, 'seconds': 0, 'rlimit_budget': 30,
                       'rlimit_run': 0, 'rlimit_init': 0}
        self.assertEqual(D['child_verdict'](stats, proof, measurement, row)['resources'], 'PASS')
        del stats['times-ms']['total']
        self.assertEqual(D['child_verdict'](stats, proof, measurement, row)['resources'], 'UNKNOWN')
        stats['verification-results']['success'] = False
        with self.assertRaises(ValueError):
            D['child_verdict'](stats, proof, measurement, row)
        stats['verification-results']['success'] = True
        stats['verification-results']['verified'] = True
        with self.assertRaises(ValueError):
            D['child_verdict'](stats, proof, measurement, row)

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
        replies = [subprocess.CompletedProcess([], 2, 'https://app.buildbuddy.io/invocation/' + parent + '\n'),
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

    def test_terminal_cas_collector_fake_wrapper(self):
        parent = '12345678-1234-1234-1234-123456789abc'
        summary = {k: 'PASS' for k in ('preparation', 'child', 'resources', 'collection', 'restoration')}
        summary.update(command_artifact_namespace='verus-measurement', reserved_name_collision=False)
        config = self.config()
        row = config.selections[0]
        summary['rows'] = [D['asdict'](row)]
        expected = D['loads']((self.evidence / 'normal-receipt.json').read_bytes())
        objects = {p.name: p.read_bytes() for p in (self.evidence / 'normal').iterdir()
                   if not p.name.endswith('pyc') and p.name != 'submission-argv.json'}
        objects.update({'measurement-summary.json': D['encoded'](summary),
            '0-test.log': b'FAKE selected child log\n',
            '0-test.xml': b'<testsuite tests="1" failures="0" errors="0"><testcase/></testsuite>',
            '0-inventory.json': D['encoded']({'proofs': {row.target: {'kind': 'verus', 'module': row.selector,
                'verified': 1, 'expected_obligations': 1, 'threads': 1, 'seconds': 0}}}),
            '0-measurement.json': D['encoded']({'label': row.target, 'verified': 1, 'seconds': 0,
                'rlimit_budget': 30, 'rlimit_run': 0, 'rlimit_init': 0}),
            '0-verifier.json': D['encoded']({'verification-results': {'verified': 1, 'errors': 0,
                'encountered-error': False, 'encountered-vir-error': False, 'is-verifying-entire-crate': False},
                'times-ms': {'total': 0, 'num-threads': 1, 'smt': {'rlimit-run': 0, 'rlimit-init': 0}}})})
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
            self.assertTrue(all(a['parent_invocation_id'] == parent for a in result['cas']))
            self.assertEqual(len(result['cas']), 3 + len(manifest))
            files.pop(S['RESERVED'])
            result = S['collect_terminal'](parent, 'PASS', D['owned_directory'](self.root / 'missing-v1'), '', config, expected)
            self.assertEqual(result['overall'], 'UNKNOWN')
            # Own terminal runner fallback, same fake object, no real scanner.
            result = S['collect_terminal'](parent, 'PASS', D['owned_directory'](self.root / 'fake-line'),
                                          S['MARKER'] + json.dumps(fake_wrapper()), config, expected)
            self.assertEqual(result['overall'], 'PASS')
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
