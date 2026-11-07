"""Offline wrapper fixtures. Execute only in an admitted CPU action."""
import base64
import hashlib
import inspect
import json
import os
from pathlib import Path
import shlex
import stat
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import bbrun


KEY = 'fixture-only-secret-+/cleanup'
RECEIPT = 'bbrun-wrapper-result.v1.json'


class CleanupTests(unittest.TestCase):
    def nested_readonly_rejection(self, *, baseline=False):
        self.assertNotEqual(os.getuid(), 0, 'root invalidates permission reproduction')
        with tempfile.TemporaryDirectory(dir="/tmp") as directory:
            root = Path(directory)
            original = root / 'original'
            original.mkdir(mode=0o700)
            (original / 'input').write_text(KEY)
            (original / 'input').chmod(0o400)
            original.chmod(0o500)
            before = (original.stat().st_mode, (original / 'input').stat().st_mode,
                      hashlib.sha256((original / 'input').read_bytes()).hexdigest())
            command_upload, upload = root / 'command', root / 'upload'
            command_upload.mkdir()
            upload.mkdir()
            nested = command_upload / 'outer' / 'inner' / 'deep'
            nested.mkdir(parents=True)
            (nested / 'copied.air').write_bytes((original / 'input').read_bytes())
            (nested / 'copied.air').chmod(0o400)
            nested.chmod(0o500)
            nested.parent.chmod(0o500)
            print(f'permission fixture uid={os.getuid()} directory={stat.S_IMODE(nested.stat().st_mode):o} file={stat.S_IMODE((nested / "copied.air").stat().st_mode):o}', flush=True)
            module = bbrun
            if baseline:
                import importlib.util
                source = (Path(__file__).with_name('fixtures') / 'bbrun-baseline89.py.txt').read_bytes()
                self.assertEqual(hashlib.sha256(source).hexdigest(), '89c8a850b9381cd2e4d52a2d94f45b5c2272c9fcb7a3708ef2a8c45552836763')
                source_file = root / 'baseline89.py'
                source_file.write_bytes(source)
                spec = importlib.util.spec_from_file_location('baseline89', source_file)
                module = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(module)
            steps = module.secured_steps(['true'], 'step', bbrun.checked_scanner())
            environment = {'PATH': os.defpath, 'BUILDBUDDY_API_KEY': KEY,
                           'BUILDBUDDY_ARTIFACTS_DIRECTORY': str(command_upload)}
            first = subprocess.run(shlex.split(steps[0]['run']), env=environment,
                                   capture_output=True, timeout=30)
            self.assertEqual(first.returncode, 0)
            environment['BUILDBUDDY_ARTIFACTS_DIRECTORY'] = str(upload)
            final = subprocess.run(shlex.split(steps[1]['run']), env=environment,
                                   capture_output=True, timeout=30)
            try:
                if baseline:
                    self.assertTrue(b'PermissionError' in final.stderr)
                    self.assertNotEqual(final.returncode, 0)
                    self.assertTrue(list(upload.iterdir()))
                    self.assertNotIn(b'BBRUN_WRAPPER_RESULT_V1 ', final.stdout)
                    print('baseline89 RED: PermissionError; residual upload; no receipt; expected failure observed', flush=True)
                    return
                self.assertFalse(b'Traceback' in final.stderr, 'wrapper exception escaped')
                self.assertNotEqual(final.returncode, 0)
                self.assertFalse(list(upload.iterdir()))
                receipt = json.loads(next(line.split(b' ', 1)[1] for line in final.stdout.splitlines()
                                          if line.startswith(b'BBRUN_WRAPPER_RESULT_V1 ')))
                self.assertEqual(receipt['command']['exit_code'], 0)
                self.assertEqual(receipt['scanner']['status'], 'fail', receipt)
                self.assertEqual(receipt['cleanup']['status'], 'pass')
                self.assertEqual(before, (original.stat().st_mode, (original / 'input').stat().st_mode,
                                         hashlib.sha256((original / 'input').read_bytes()).hexdigest()))
                self.assertNotIn(KEY.encode(), first.stdout + first.stderr + final.stdout + final.stderr)
            finally:
                # Fixture teardown only, restricted to this fixture's owned tree.
                for parent, dirs, _ in os.walk(root):
                    Path(parent).chmod(0o700)
                    for name in dirs:
                        Path(parent, name).chmod(0o700)

    def test_nested_readonly_rejection(self):
        self.nested_readonly_rejection()

    def test_baseline89_red(self):
        self.nested_readonly_rejection(baseline=True)


class ReceiptTests(unittest.TestCase):
    def fixture(self, *, command=0, content='harmless', scanner=None, injection='', state_change=None,
                reserved=False, symlink=False, unavailable=False, extracted=False, missing_key=False, symlink_root=False,
                no_staging=False, first_injection='', full_history=False, first_upload_mode=None):
        with tempfile.TemporaryDirectory(dir='/tmp') as directory:
            root = Path(directory)
            first_upload, upload = root / 'command', root / 'upload'
            first_upload.mkdir()
            upload.mkdir()
            original = root / 'external'
            original.mkdir()
            original_file = original / 'input'
            original_file.write_text('external unchanged')
            original_file.chmod(0o400)
            if symlink:
                (first_upload / 'link').symlink_to(original, target_is_directory=True)
            (first_upload / (RECEIPT if reserved else 'proof')).write_bytes(content if isinstance(content, bytes) else content.encode())
            if first_upload_mode is not None:
                self.assertNotEqual(os.getuid(), 0, 'root invalidates permission reproduction')
                first_upload.chmod(first_upload_mode)
            data = bbrun.checked_scanner() if scanner is None else scanner
            with patch.object(bbrun, 'SCANNER_SHA256', hashlib.sha256(data).hexdigest()):
                program = f'import os; os.kill(os.getpid(), {-command})' if command < 0 else f'import sys; sys.exit({command})'
                command_argv = [sys.executable, '-I', '-S', '-c', program]
                steps = bbrun.secured_steps(command_argv, 'step', data, 'a' * 40 if full_history else None)
                if extracted:
                    helpers = (bbrun.RemoteError, bbrun.ActionTooLarge, bbrun.check_action_size,
                               bbrun.BuildBuddy, bbrun.child_env, bbrun.run_and_record,
                               bbrun.scan_artifacts, bbrun.secured_steps)
                    renderer = root / 'kani-extracted.py'
                    source = 'import base64,contextlib,inspect,json,os,shlex,sys,urllib.request,urllib.error,uuid\nfrom pathlib import Path\n'
                    source += 'SCANNER_SHA256=' + repr(hashlib.sha256(data).hexdigest()) + '\n'
                    source += '\n'.join(inspect.getsource(fn) for fn in helpers)
                    source += '\nprint(json.dumps(secured_steps(' + repr(command_argv) + ', "step", ' + repr(data) + ')))\n'
                    renderer.write_text(source)
                    rendered = subprocess.run([sys.executable, '-I', '-S', str(renderer)],
                                              env={'PATH': os.defpath}, capture_output=True, timeout=30)
                    self.assertEqual(rendered.returncode, 0, 'Kani extraction rendering failed')
                    steps = json.loads(rendered.stdout)
            # The real upload and private status have different parents. A
            # same-parent rename would hide the readonly-root permission gap.
            (root / 'runner').mkdir()
            result_path = str(root / 'runner' / 'result.status')
            argv = [shlex.split(step['run']) for step in steps]
            import re
            for arguments in argv:
                arguments[4] = re.sub(r'/tmp/bbrun-[0-9a-f]+\.status', result_path, arguments[4])
            env = {'PATH': os.defpath, 'BUILDBUDDY_API_KEY': KEY,
                   'BUILDBUDDY_ARTIFACTS_DIRECTORY': str(first_upload)}
            if first_injection or full_history:
                source = argv[0][4]
                call = source.rfind('\nrun_with_full_history(' if full_history else '\nrun_and_record(')
                injection_source = 'prepare_full_history = lambda commit: False\n' if full_history else first_injection
                argv[0][4] = source[:call] + '\n' + injection_source + source[call:]
            first = subprocess.run(argv[0], env=env, capture_output=True, timeout=30)
            self.assertEqual(first.returncode, 0)
            self.assertFalse(list(first_upload.iterdir()), 'unscanned command upload')
            state_path = Path(result_path + '.private') / 'command.json'
            if state_change:
                state_change(state_path)
            if no_staging:
                staged = Path(result_path + '.artifacts')
                (staged / 'proof').unlink()
                staged.rmdir()
            if missing_key:
                env.pop('BUILDBUDDY_API_KEY')
            if symlink_root:
                upload.rmdir()
                upload.symlink_to(original, target_is_directory=True)
            if unavailable:
                (upload / RECEIPT).symlink_to(original_file)
            if injection:
                source = argv[1][4]
                call = source.rfind('\nscan_artifacts(')
                argv[1][4] = source[:call] + '\n' + injection + source[call:]
            env['BUILDBUDDY_ARTIFACTS_DIRECTORY'] = str(upload)
            final = subprocess.run(argv[1], env=env, capture_output=True, timeout=30)
            output = first.stdout + first.stderr + final.stdout + final.stderr
            self.assertNotIn(b'Traceback', output)
            from urllib.parse import quote, quote_plus
            forms = [KEY, quote(KEY), quote(KEY, safe=''), quote_plus(KEY),
                     base64.b64encode(KEY.encode()).decode(),
                     base64.urlsafe_b64encode(KEY.encode()).decode()]
            for value in forms:
                self.assertNotIn(value.encode(), output)
                for path in upload.rglob('*'):
                    if path.is_file():
                        self.assertNotIn(value.encode(), path.read_bytes())
            lines = [line[len(b'BBRUN_WRAPPER_RESULT_V1 '):] for line in final.stdout.splitlines()
                     if line.startswith(b'BBRUN_WRAPPER_RESULT_V1 ')]
            self.assertEqual(len(lines), 1)
            receipt = json.loads(lines[0])
            self.assertEqual(receipt['wrapper']['exit_code'], final.returncode)
            self.assertEqual(receipt['schema'], 'tl3.bbrun.wrapper-result')
            self.assertEqual(receipt['version'], 1)
            self.assertIsNone(receipt['parent_invocation_id'])
            # Match the actual schema's required fields and reject extra fields.
            schema = json.loads(Path(bbrun.__file__).with_name('bbrun-wrapper-result.v1.schema.json').read_text())
            self.assertEqual(set(receipt), set(schema['required']))
            for stage in ('command', 'artifact_handoff', 'scanner', 'cleanup', 'wrapper'):
                self.assertEqual(set(receipt[stage]), set(schema['properties'][stage]['required']))
            published = upload / RECEIPT
            if published.exists():
                self.assertEqual(json.loads(published.read_bytes()), receipt)
            self.assertEqual(original_file.read_text(), 'external unchanged')
            self.assertEqual(stat.S_IMODE(original_file.stat().st_mode), 0o400)
            names = sorted(path.name for path in upload.iterdir())
            self.assertEqual(Path(result_path).read_text(), str(final.returncode))
            return receipt, names, output

    def test_normal_and_nonzero_command(self):
        for code in (0, 37, -15):
            receipt, names, _ = self.fixture(command=code)
            actual = code if code >= 0 else 128 - code
            self.assertEqual(receipt['command']['exit_code'], code)
            self.assertEqual(receipt['wrapper'], {'original_exit_code': actual, 'exit_code': actual})
            self.assertEqual(receipt['scanner']['status'], 'pass')
            self.assertEqual(receipt['cleanup'], {'status': 'pass', 'targets_present': True, 'reason': 'cleanup_complete'})
            self.assertEqual(names, [RECEIPT, 'proof'])

    def test_scanner_nonzero_cleanup_failure_and_original(self):
        for scanner_code in (1, 42, 75):
            receipt, names, _ = self.fixture(scanner=f'raise SystemExit({scanner_code})\n'.encode(), injection='''import shutil
_original_rmtree = shutil.rmtree
def denied(*args, **kwargs):
    raise PermissionError('fixture-only-secret-+/cleanup')
denied.avoids_symlink_attacks = _original_rmtree.avoids_symlink_attacks
shutil.rmtree = denied
''')
            self.assertEqual(receipt['command']['exit_code'], 0)
            self.assertEqual(receipt['scanner'], {'status': 'fail', 'first_exit_code': scanner_code,
                                                'exit_code': scanner_code, 'reason': 'scan_rejected'})
            self.assertEqual(receipt['cleanup']['status'], 'fail')
            self.assertEqual(receipt['cleanup']['reason'], 'cleanup_permission_denied')
            self.assertEqual(receipt['wrapper'], {'original_exit_code': scanner_code, 'exit_code': scanner_code})
            self.assertEqual(names, [])

    def test_scanner_exception_no_invented_code(self):
        receipt, names, _ = self.fixture(scanner=b"raise RuntimeError('fixture-only-secret-+/cleanup')\n")
        self.assertEqual(receipt['scanner'], {'status': 'error', 'first_exit_code': None,
                                            'exit_code': None, 'reason': 'scanner_exception'})
        self.assertEqual(receipt['wrapper']['exit_code'], 75)
        self.assertEqual(names, [])

    def test_missing_private_command_and_cleanup(self):
        def missing(path):
            path.unlink()
        def unknown(path):
            value = json.loads(path.read_bytes())
            value['cleanup'] = {'status': 'unknown', 'targets_present': None, 'reason': 'cleanup_not_observed'}
            path.write_text(json.dumps(value))
        def contradictory(path):
            value = json.loads(path.read_bytes())
            value['cleanup']['targets_present'] = True
            path.write_text(json.dumps(value))
        for change in (missing, unknown, contradictory):
            receipt, names, _ = self.fixture(state_change=change)
            self.assertNotEqual(receipt['wrapper']['exit_code'], 0)
            self.assertNotIn(RECEIPT, names)
            if change != unknown:
                self.assertEqual(receipt['command']['status'], 'unknown')
            self.assertEqual(receipt['cleanup']['status'], 'unknown')

    def test_rejected_secrets_and_encodings(self):
        from urllib.parse import quote
        for secret in (KEY, base64.b64encode(KEY.encode()).decode(), quote(KEY, safe='')):
            receipt, names, _ = self.fixture(content=secret)
            self.assertEqual(receipt['scanner']['status'], 'fail')
            self.assertNotEqual(receipt['wrapper']['exit_code'], 0)
            self.assertEqual(names, [])
            self.assertEqual(receipt['cleanup']['status'], 'pass')

    def test_reserved_collision_and_symlink(self):
        for arguments in ({'reserved': True}, {'symlink': True}, {'unavailable': True}, {'symlink_root': True}):
            receipt, names, _ = self.fixture(**arguments)
            self.assertNotEqual(receipt['wrapper']['exit_code'], 0)
            self.assertEqual(names, [])
            if not arguments.get('symlink_root'):
                self.assertEqual(receipt['artifact_handoff']['status'], 'fail')

    def test_receipt_unavailable(self):
        receipt, names, _ = self.fixture(injection='''import os
def unavailable(*args, **kwargs):
    raise PermissionError('fixture-only-secret-+/cleanup')
os.link = unavailable
''')
        self.assertEqual(receipt['wrapper'], {'original_exit_code': 0, 'exit_code': 75})
        self.assertEqual(receipt['scanner']['status'], 'pass')
        self.assertEqual(names, [])

    def test_extracted_kani_source(self):
        namespace = {}
        exec('import base64,contextlib,inspect,json,os,shlex,sys,urllib.request,urllib.error,uuid\nfrom pathlib import Path\nfrom unittest.mock import patch', namespace)
        for name in ('API', 'MAX_SERIALIZED_ACTION_BYTES', 'SCANNER_SHA256'):
            namespace[name] = getattr(bbrun, name)
        helpers = (bbrun.RemoteError, bbrun.ActionTooLarge, bbrun.check_action_size,
                   bbrun.BuildBuddy, bbrun.child_env, bbrun.run_and_record,
                   bbrun.scan_artifacts, bbrun.secured_steps)
        exec(compile('\n'.join(inspect.getsource(fn) for fn in helpers), '<kani-extracted>', 'exec'), namespace)
        # inspect.getsource on extracted definitions is exercised using the
        # original module functions, exactly as captain's patched import does.
        for fn in (bbrun.run_and_record, bbrun.scan_artifacts):
            self.assertNotIn('from bbrun', inspect.getsource(fn))
        self.assertIn('run_and_record', namespace)
        self.fixture(extracted=True)

    def test_cleanup_boundary(self):
        with tempfile.TemporaryDirectory(dir='/tmp') as directory:
            root = Path(directory)
            outside = root / 'outside'
            outside.mkdir()
            (outside / 'input').write_text('unchanged')
            outside.chmod(0o500)
            owned = root / 'status.artifacts'
            owned.mkdir()
            (owned / 'link').symlink_to(outside, target_is_directory=True)
            support = bbrun.run_and_record(None, None, str(root / 'status'))
            support.remove(owned)
            self.assertFalse(owned.exists())
            link = owned
            link.symlink_to(outside, target_is_directory=True)
            with self.assertRaises((ValueError, OSError)):
                support.remove(link)
            with self.assertRaises((ValueError, OSError)):
                support.remove(owned / '..' / 'outside')
            self.assertTrue((outside / 'input').exists())
            self.assertEqual(stat.S_IMODE(outside.stat().st_mode), 0o500)
            outside.chmod(0o700)

    def test_no_targets_and_pre_execution_error(self):
        receipt, names, _ = self.fixture(no_staging=True)
        self.assertEqual(receipt['cleanup'], {'status': 'not_needed', 'targets_present': False, 'reason': 'no_cleanup_targets'})
        self.assertEqual(names, [RECEIPT])
        receipt, names, _ = self.fixture(missing_key=True)
        self.assertIsNone(receipt['scanner']['first_exit_code'])
        self.assertIsNone(receipt['scanner']['exit_code'])
        self.assertEqual(receipt['scanner']['status'], 'error')
        self.assertNotEqual(receipt['wrapper']['exit_code'], 0)
        self.assertEqual(names, [])

    def test_handoff_and_full_history_failure(self):
        cases = ({'full_history': True}, {'first_injection': "import shutil\ndef denied(*args, **kwargs):\n    raise PermissionError('fixture-only-secret-+/cleanup')\nshutil.move = denied\n"})
        for arguments in cases:
            receipt, names, _ = self.fixture(**arguments)
            self.assertEqual(receipt['artifact_handoff']['status'], 'fail')
            self.assertEqual(receipt['wrapper']['exit_code'], 75)
            self.assertEqual(receipt['command']['exit_code'], None if arguments.get('full_history') else 0)
            self.assertEqual(receipt['scanner'], {'status': 'error', 'first_exit_code': None,
                                                'exit_code': None, 'reason': 'scanner_not_run'})
            self.assertEqual(names, [])

    def test_readonly_command_upload_root(self):
        receipt, names, _ = self.fixture(content=KEY, first_upload_mode=0o500)
        self.assertEqual(receipt['artifact_handoff']['status'], 'pass')
        self.assertEqual(receipt['scanner']['status'], 'fail')
        self.assertEqual(receipt['cleanup']['status'], 'pass')
        self.assertNotEqual(receipt['wrapper']['exit_code'], 0)
        self.assertEqual(names, [])

    def test_readonly_rejected_upload_root(self):
        self.assertNotEqual(os.getuid(), 0, 'root invalidates permission reproduction')
        scanner = (b'import os,sys\nos.chmod(sys.argv[-1], 0o500)\nexec('
                   + repr(bbrun.checked_scanner()).encode() + b')\n')
        receipt, names, _ = self.fixture(content=KEY, scanner=scanner)
        self.assertEqual(receipt['scanner']['status'], 'fail')
        self.assertEqual(receipt['cleanup']['status'], 'pass')
        self.assertNotEqual(receipt['wrapper']['exit_code'], 0)
        self.assertEqual(names, [])

    def test_final_handoff_failure_preserves_command(self):
        receipt, names, _ = self.fixture(injection="import shutil\ndef denied(*args, **kwargs):\n    raise PermissionError('fixture-only-secret-+/cleanup')\nshutil.move = denied\n")
        self.assertEqual(receipt['command']['exit_code'], 0)
        self.assertEqual(receipt['artifact_handoff']['status'], 'fail')
        self.assertIsNone(receipt['scanner']['first_exit_code'])
        self.assertIsNone(receipt['scanner']['exit_code'])
        self.assertEqual(receipt['cleanup']['status'], 'pass')
        self.assertEqual(receipt['wrapper']['exit_code'], 75)
        self.assertEqual(names, [])

    def test_permitted_drop_preserves_first_code(self):
        receipt, names, _ = self.fixture(scanner=b'''import sys
from pathlib import Path
root = Path(sys.argv[-1])
if root.is_dir() and (root / 'proof').exists():
    print(str(root / 'proof') + ': zstd: not scanned (compression.zstd unavailable)')
    raise SystemExit(75)
raise SystemExit(0)
''')
        self.assertEqual(receipt['scanner'], {'status': 'pass', 'first_exit_code': 75,
                                            'exit_code': 0, 'reason': 'scan_clean'})
        self.assertEqual(receipt['wrapper']['exit_code'], 0)
        self.assertEqual(receipt['cleanup']['status'], 'pass')
        self.assertEqual(names, [RECEIPT])

    def test_cleanup_onerror_compatibility_and_root_swap(self):
        import shutil
        with tempfile.TemporaryDirectory(dir='/tmp') as directory:
            root = Path(directory)
            owned = root / 'status.artifacts'
            (owned / 'inner' / 'deep').mkdir(parents=True)
            (owned / 'inner' / 'deep' / 'input').write_text('fixture')
            (owned / 'inner' / 'deep').chmod(0o500)
            (owned / 'inner').chmod(0o500)
            support = bbrun.run_and_record(None, None, str(root / 'status'))
            actual = shutil.rmtree
            def legacy(path, *, dir_fd, onerror):
                return actual(path, dir_fd=dir_fd, onerror=onerror)
            legacy.avoids_symlink_attacks = actual.avoids_symlink_attacks
            with patch('shutil.rmtree', legacy):
                support.remove(owned)
            self.assertFalse(owned.exists())
            outside = root / 'outside'
            outside.mkdir()
            (outside / 'input').write_text('unchanged')
            owned.mkdir()
            def swapped(path, *, dir_fd, onerror):
                owned.rename(root / 'saved')
                owned.symlink_to(outside, target_is_directory=True)
                return actual(path, dir_fd=dir_fd, onerror=onerror)
            swapped.avoids_symlink_attacks = actual.avoids_symlink_attacks
            with patch('shutil.rmtree', swapped), self.assertRaises((ValueError, OSError)):
                support.remove(owned)
            self.assertEqual((outside / 'input').read_text(), 'unchanged')


if __name__ == '__main__':
    if sys.argv[1:] == ['--qualify']:
        suite = unittest.defaultTestLoader.discover(str(Path(__file__).parent), pattern='test_bbrun*.py')
        outcome = unittest.TextTestRunner(verbosity=2).run(suite)
        if not outcome.wasSuccessful():
            raise SystemExit(1)
        with patch.dict(os.environ, dict(os.environ), clear=True):
            if bbrun.selftest() not in (None, 0):
                raise SystemExit(1)
        subprocess.run(['/home/rob/.local/bin/uv', 'tool', 'run', '--from', 'ruff==0.14.14', 'ruff', 'check',
                        '--isolated', '--select', 'E9,F', 'tools/ci/bbrun.py',
                        'tools/ci/test_bbrun_cleanup.py', 'tools/ci/test_bbrun_cancel.py',
                        'tools/ci/test_bbrun_scan_diagnostics.py'], check=True)
    else:
        unittest.main()
