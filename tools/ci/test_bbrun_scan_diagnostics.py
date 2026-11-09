"""Offline rejection evidence fixtures; run only through admitted compute."""
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import quote, quote_plus

import bbrun


class ScanDiagnosticsTests(unittest.TestCase):
    def fixture(self, *, filename='proof.air', mixed=False, clean=False, scanner=None,
                encoding='raw', second_file=False, reject=None, filename_encoding='url', missing_key=False):
        # Never give unittest the planted value as an assertion operand.
        value = 'TL3FAKE-' + secrets.token_urlsafe(32) + '+/=?'
        forms = {value, quote(value), quote(value, safe=''), quote_plus(value)}
        for encoded in (base64.b64encode(value.encode()), base64.urlsafe_b64encode(value.encode())):
            forms.update((encoded.decode(), encoded.rstrip(b'=').decode()))
        forms.update(re.sub(r'%[0-9A-F]{2}', lambda m: m[0].lower(), form) for form in tuple(forms))
        with tempfile.TemporaryDirectory(prefix='bbrun-diagnostics-') as directory:
            root = Path(directory)
            command_upload, upload = root / 'command', root / 'upload'
            command_upload.mkdir()
            upload.mkdir()
            filename_value = {'url': quote(value, safe=''), 'raw': value,
                              'base64': base64.b64encode(value.encode()).decode()}[filename_encoding]
            name = filename.replace('SECRET', filename_value)
            (command_upload / name).parent.mkdir(parents=True, exist_ok=True)
            encoded = {'raw': value, 'base64': base64.b64encode(value.encode()).decode(),
                       'url': quote(value, safe='')}[encoding]
            payload = b'innocent' if clean else b'CONTEXT-MUST-NOT-REMAIN ' + encoded.encode() * 3
            (command_upload / name).write_bytes(payload)
            if second_file:
                (command_upload / 'other.air').write_bytes(payload)
            if mixed:
                (command_upload / 'broken.gz').write_bytes(b'\x1f\x8bbroken')
            data = bbrun.checked_scanner() if scanner is None else scanner
            with patch.object(bbrun, 'SCANNER_SHA256', hashlib.sha256(data).hexdigest()):
                steps = bbrun.secured_steps([sys.executable, '-I', '-S', '-c', 'pass'], 'step', data)
            argv = [shlex.split(step['run']) for step in steps]
            result_path = str(root / 'status')
            for args in argv:
                args[4] = re.sub(r'/tmp/bbrun-[0-9a-f]+\.status', result_path, args[4])
            env = {'PATH': os.defpath, 'BUILDBUDDY_API_KEY': value,
                   'BUILDBUDDY_ARTIFACTS_DIRECTORY': str(command_upload)}
            first = subprocess.run(argv[0], env=env, capture_output=True, timeout=30)
            self.assertEqual(first.returncode, 0)
            env['BUILDBUDDY_ARTIFACTS_DIRECTORY'] = str(upload)
            if missing_key:
                env.pop('BUILDBUDDY_API_KEY')
            final = subprocess.run(argv[1], env=env, capture_output=True, timeout=30)
            output = first.stdout + first.stderr + final.stdout + final.stderr
            retained = output + b''.join(p.read_bytes() for p in Path(result_path + '.private').rglob('*') if p.is_file())
            retained += b''.join(p.read_bytes() for p in upload.rglob('*') if p.is_file())
            self.assertTrue(all(form.encode() not in retained for form in forms), 'credential form retained')
            self.assertTrue(b'CONTEXT-MUST-NOT-REMAIN' not in retained, 'matched context retained')
            self.assertTrue(final.stdout.splitlines()[-1].startswith(b'BBRUN_WRAPPER_RESULT_V1 '), 'final receipt missing')
            receipt = json.loads(final.stdout.splitlines()[-1].split(b' ', 1)[1])
            records = [json.loads(line.split(b' ', 1)[1]) for line in final.stdout.splitlines()
                       if line.startswith(b'BBRUN_SCAN_DIAGNOSTICS_V1 ')]
            rejected = reject if reject is not None else not clean
            if rejected:
                self.assertNotEqual(final.returncode, 0)
                self.assertFalse(bool(list(upload.iterdir())), 'rejected bytes retained')
                self.assertIn(receipt['scanner']['status'], ('fail', 'error'))
                evidence = Path(result_path + '.private') / 'scan-diagnostics.v1.json'
                if records:
                    self.assertTrue(evidence.is_file(), 'private evidence missing')
                    self.assertEqual(json.loads(evidence.read_bytes()), records)
            return records, receipt

    def test_real_scanner_rejection_evidence(self):
        records, receipt = self.fixture()
        self.assertEqual(receipt['scanner']['exit_code'], 1)
        self.assertTrue(bool(records), 'rejection retained no path/rule diagnostics')
        self.assertEqual(records[0]['detections'], [{'path': 'proof.air', 'rule_id': 'BUILDBUDDY_API_KEY', 'match_count': 1}])
        self.assertEqual(records[0]['diagnostics_status'], 'complete')

    def test_clean(self):
        records, receipt = self.fixture(clean=True)
        self.assertEqual(records, [])
        self.assertEqual(receipt['scanner']['status'], 'pass')

    def test_mixed_incomplete(self):
        records, receipt = self.fixture(mixed=True)
        self.assertEqual(receipt['scanner']['first_exit_code'], 75)
        self.assertTrue(bool(records), 'mixed rejection lacks evidence')
        self.assertEqual(records[0]['detections'][0]['match_count'], 1)
        self.assertEqual(records[0]['incomplete_report_count'], 1)

    def test_encoded_and_multiple_files(self):
        for encoding in ('base64', 'url'):
            records, _ = self.fixture(encoding=encoding, second_file=True)
            self.assertTrue(bool(records), 'encoded rejection lacks evidence')
            self.assertEqual(sorted(item['path'] for item in records[0]['detections']), ['other.air', 'proof.air'])
            self.assertEqual([item['match_count'] for item in records[0]['detections']], [1, 1])

    def test_sensitive_and_ambiguous_names(self):
        for name in ('SECRET.air', 'colon:name.air', 'line\nbreak.air'):
            records, _ = self.fixture(filename=name)
            self.assertTrue(bool(records), 'unsafe-name rejection lacks evidence')
            self.assertTrue(all(item['path'] is None or item['path'] == '.air'
                                for item in records[0]['detections']), 'unsafe path emitted')

    def test_raw_and_base64_sensitive_names(self):
        for encoding in ('raw', 'base64'):
            records, _ = self.fixture(filename='SECRET.air', filename_encoding=encoding)
            self.assertTrue(bool(records), 'sensitive-name rejection lacks evidence')
            self.assertEqual(records[0]['detections'][0]['path'], '.air')

    def test_unknown_diagnostics(self):
        records, _ = self.fixture(scanner=b"print('unrecognized output'); raise SystemExit(1)\n")
        self.assertTrue(bool(records), 'unknown rejection lacks evidence')
        self.assertEqual(records[0]['detections'], [])
        self.assertEqual(records[0]['diagnostics_status'], 'partial')

    def test_error_and_unavailable_diagnostics(self):
        source = b"import os; print(os.environ['BUILDBUDDY_API_KEY']); raise RuntimeError(os.environ['BUILDBUDDY_API_KEY'])\n"
        records, receipt = self.fixture(scanner=source)
        self.assertEqual(receipt['scanner']['status'], 'error')
        self.assertTrue(bool(records), 'scanner exception lacks evidence')
        self.assertEqual(records[0]['diagnostics_status'], 'partial')
        self.assertEqual(records[0]['detections'], [])
        records, receipt = self.fixture(missing_key=True)
        self.assertEqual(receipt['scanner']['status'], 'error')
        self.assertTrue(bool(records), 'missing scanner lacks evidence')
        self.assertEqual(records[0]['diagnostics_status'], 'unavailable')

    def test_mixed_unknown_and_duplicate_reports(self):
        source = b'''import sys
report = f'{sys.argv[-1]}/proof.air: BUILDBUDDY_API_KEY: secret detected'
print(report)
print(report)
print('unknown diagnostic')
raise SystemExit(1)
'''
        records, _ = self.fixture(scanner=source)
        self.assertTrue(bool(records), 'mixed unknown rejection lacks evidence')
        self.assertEqual(records[0]['diagnostics_status'], 'partial')
        self.assertEqual(records[0]['detections'], [{'path': 'proof.air', 'rule_id': 'BUILDBUDDY_API_KEY', 'match_count': 1}])

    def test_second_scan_and_receipt_rejection(self):
        template = '''import os, sys
from pathlib import Path
upload = Path(os.environ['BUILDBUDDY_ARTIFACTS_DIRECTORY'])
counter = upload.parent / 'calls'
count = int(counter.read_text()) + 1 if counter.exists() else 1
counter.write_text(str(count))
if count == 1:
    FIRST
print(f'{Path(sys.argv[-1]) if RECEIPT else upload / "other.air"}: BUILDBUDDY_API_KEY: secret detected')
raise SystemExit(1)
'''
        for receipt in (False, True):
            first = 'raise SystemExit(0)' if receipt else 'print(f\'{upload / "proof.air"}: zstd: not scanned (compression.zstd unavailable)\'); raise SystemExit(75)'
            source = template.replace('FIRST', first).replace('RECEIPT', repr(receipt)).encode()
            records, result = self.fixture(clean=True, scanner=source, reject=True, second_file=True)
            self.assertEqual(result['scanner']['exit_code'], 1)
            self.assertTrue(bool(records), 'later rejection lacks evidence')
            self.assertEqual(records[-1]['phase'], 'receipt' if receipt else 'remaining')
            self.assertEqual(records[-1]['diagnostics_status'], 'complete')
            self.assertEqual(records[-1]['detections'][0]['path'], None if receipt else 'other.air')


if __name__ == '__main__':
    unittest.main()
