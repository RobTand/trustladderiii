"""Exercise upload commands with fake secrets and lint every workflow upload."""
import contextlib
import gzip
import io
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import yaml

import bazel_gate
import remote_gate
from child_env import child_env

ROOT = Path(__file__).resolve().parents[2]
REQUIRED = ('BUILDBUDDY_API_KEY', 'MERGIFY_TOKEN')
FORK_MESSAGE = 'artifact uploads skipped: fork PR has no secrets to scan for'


def workflows():
    for path in sorted((ROOT / '.github/workflows').glob('*.y*ml')):
        yield path, yaml.safe_load(path.read_text())


def is_scan(step):
    return re.search(r'\bpython3?\s+tools/ci/(before_upload|check_no_secret_leak)\.py\b',
                     step.get('run', '')) is not None


def enabled(step, *, fork=False, outcomes=None):
    expression = str(step.get('if', 'true')).removeprefix('${{').removesuffix('}}').strip()
    values = {'always()': True, 'github.event_name': 'pull_request',
              'github.event.pull_request.head.repo.fork': fork,
              'true': True, 'false': False}
    values.update({f'steps.{name}.outcome': value for name, value in (outcomes or {}).items()})
    for name, value in values.items():
        expression = re.sub(re.escape(name) + r'(?![\w.])', repr(value), expression)
    return eval(expression.replace('&&', ' and ').replace('||', ' or '), {'__builtins__': {}})


class WorkflowScanTests(unittest.TestCase):
    def test_every_artifact_upload_requires_both_scoped_secrets(self):
        uploads = 0
        for path, workflow in workflows():
            for job in workflow['jobs'].values():
                steps = job.get('steps', [])
                for index, upload in enumerate(steps):
                    if not upload.get('uses', '').startswith('actions/upload-artifact@'):
                        continue
                    uploads += 1
                    with self.subTest(workflow=path.name, artifact=upload['with']['name']):
                        scan = steps[index - 1]
                        self.assertTrue(is_scan(scan))
                        for name in REQUIRED:
                            self.assertIn(f'--require {name}', scan['run'])
                            self.assertEqual(scan['env'][name], '${{ secrets.' + name + ' }}')
                        self.assertFalse(upload.get('continue-on-error', False))
                        self.assertTrue(enabled(upload, outcomes={scan['id']: 'success'}))
                        for outcome in ('failure', 'skipped', 'cancelled'):
                            self.assertFalse(enabled(upload, outcomes={scan['id']: outcome}))
        self.assertGreater(uploads, 0)

    def test_fork_pr_skips_scans_and_uploads_and_logs_why(self):
        for path, workflow in workflows():
            for job in workflow['jobs'].values():
                steps = job.get('steps', [])
                uploads = [step for step in steps
                           if step.get('uses', '').startswith('actions/upload-artifact@')]
                if not uploads:
                    continue
                with self.subTest(workflow=path.name, job=job.get('name')):
                    notices = [step for step in steps if FORK_MESSAGE in step.get('run', '')]
                    self.assertEqual(len(notices), 1)
                    self.assertTrue(enabled(notices[0], fork=True))
                    self.assertFalse(enabled(notices[0], fork=False))
                    for step in steps:
                        if step in uploads or is_scan(step):
                            self.assertFalse(enabled(step, fork=True, outcomes={'secret-check': 'success'}))

    def test_workflow_upload_commands_fail_on_missing_or_planted_secrets(self):
        for path, workflow in workflows():
            for job in workflow['jobs'].values():
                for step in job.get('steps', []):
                    if not is_scan(step):
                        continue
                    for missing in (*REQUIRED, None):
                        for empty in (False, True) if missing else (False,):
                            with self.subTest(workflow=path.name, missing=missing, empty=empty), \
                                 tempfile.TemporaryDirectory() as directory:
                                env = child_env()
                                env.update({name: 'fake-upload-value-' + name for name in REQUIRED})
                                env['GITHUB_TOKEN'] = 'fake-github-upload-value'
                                artifact = Path(directory, 'gate-output')
                                artifact.mkdir()
                                (artifact / 'log.txt').write_text(
                                    env['BUILDBUDDY_API_KEY'] if missing is None else 'clean log')
                                if missing:
                                    if empty:
                                        env[missing] = ''
                                    else:
                                        env.pop(missing)
                                command = step['run'].replace('python3 ', f'{sys.executable} ', 1)
                                # Use the workflow's exact arguments with an absolute script path.
                                command = command.replace('tools/ci/', str(ROOT / 'tools/ci') + '/')
                                result = subprocess.run(command, shell=True, cwd=directory,
                                                        env=env, text=True, capture_output=True,
                                                        executable='/bin/bash')
                                self.assertEqual(result.returncode, 75 if missing else 1,
                                                 result.stdout + result.stderr)
                                for name in REQUIRED:
                                    self.assertNotIn('fake-upload-value-' + name,
                                                     result.stdout + result.stderr)


class RunnerScanTests(unittest.TestCase):
    def test_runner_excludes_unscannable_files_and_rescans(self):
        secret = 'fake-runner-secret-value'
        nested = b'clean'
        for _ in range(34):
            nested = gzip.compress(nested)
        cases = [('partial.zst', bytes.fromhex('28b52ffdff'), 'zstd: not scanned'),
                 (secret + '.zst', bytes.fromhex('28b52ffdff'), 'zstd: not scanned'),
                 ('broken.gz', b'\x1f\x8b\x08broken', 'gzip member unreadable'),
                 ('nested.gz', nested, 'compressed nesting limit')]
        for name, payload, reason in cases:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                artifacts = Path(directory, 'artifacts')
                artifacts.mkdir()
                skipped = artifacts / name
                skipped.write_bytes(payload)
                (artifacts / 'gate-result.json').write_text('{}')
                calls = []
                real_run = subprocess.run

                def scan(command, **kwargs):
                    calls.append(command)
                    # Run the real upload hook with compression imports unavailable.
                    code = ('import runpy, sys; sys.modules["compression"] = None; '
                            'sys.modules["compression.zstd"] = None; '
                            'sys.argv.pop(0); runpy.run_path(sys.argv[0], run_name="__main__")')
                    kwargs['capture_output'] = True
                    kwargs['text'] = True
                    return real_run([*command[:3], '-c', code, *command[3:]], **kwargs)

                output = io.StringIO()
                with patch.dict(os.environ, {'BUILDBUDDY_API_KEY': secret}, clear=True), \
                     patch.object(bazel_gate.subprocess, 'run', side_effect=scan), \
                     contextlib.redirect_stdout(output):
                    status = bazel_gate.scan_artifacts(artifacts)
                self.assertEqual(status, 0, output.getvalue())
                self.assertFalse(skipped.exists(), output.getvalue())
                self.assertEqual((artifacts / 'gate-result.json').read_text(), '{}')
                self.assertEqual(len(calls), 2)
                self.assertIn(repr(name.replace(secret, '')), output.getvalue())
                self.assertIn(f'{len(payload)} bytes', output.getvalue())
                self.assertIn(reason, output.getvalue())
                self.assertNotIn(secret, output.getvalue())

    def test_runner_rejects_secret_hits_and_unknown_scan_errors(self):
        for code, diagnostic in ((1, 'secret detected'), (42, ''), (0, 'unknown error'),
                                 (75, 'unknown error'), (75, 'unreadable directory, not scanned'),
                                 (75, 'secret detected')):
            with self.subTest(code=code, diagnostic=diagnostic), \
                 tempfile.TemporaryDirectory() as directory:
                artifacts = Path(directory)
                skipped = artifacts / 'partial.zst'
                skipped.write_bytes(b'opaque')
                report = f'{skipped}: zstd: not scanned (compression.zstd unavailable)\n'
                result = subprocess.CompletedProcess([], code, report + diagnostic + '\n', '')
                with patch.dict(os.environ, {}, clear=True), \
                     patch.object(bazel_gate.subprocess, 'run', return_value=result) as run, \
                     contextlib.redirect_stdout(io.StringIO()):
                    status = bazel_gate.scan_artifacts(artifacts)
                self.assertNotEqual(status, 0)
                self.assertEqual(run.call_count, 1)
                self.assertEqual(list(artifacts.iterdir()), [])

    def test_runner_requires_a_clean_second_scan(self):
        for code, output in ((1, 'secret detected'), (42, ''), (0, 'unknown diagnostic'),
                             (75, 'not scanned'), (0, 'not scanned')):
            with self.subTest(code=code, output=output), tempfile.TemporaryDirectory() as directory:
                artifacts = Path(directory)
                skipped = artifacts / 'partial.zst'
                skipped.write_bytes(b'opaque')
                (artifacts / 'gate-result.json').write_text('{}')
                first = subprocess.CompletedProcess([], 75,
                    f'{skipped}: zstd: not scanned (compression.zstd unavailable)\n', '')
                second = subprocess.CompletedProcess([], code, output, '')
                with patch.dict(os.environ, {}, clear=True), \
                     patch.object(bazel_gate.subprocess, 'run', side_effect=[first, second]) as run, \
                     contextlib.redirect_stdout(io.StringIO()):
                    status = bazel_gate.scan_artifacts(artifacts)
                self.assertNotEqual(status, 0)
                self.assertEqual(run.call_count, 2)
                self.assertEqual(list(artifacts.iterdir()), [])

    def test_scan_supervisor_scrubs_the_gate_child_and_scans_after_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            artifacts = Path(directory, 'artifacts')
            script = Path(directory, 'fake_gate.py')
            script.write_text(
                'import os, pathlib, sys\n'
                'assert sys.argv[1] == "--runner-child"\n'
                'assert "BUILDBUDDY_API_KEY" not in os.environ\n'
                'assert "MERGIFY_TOKEN" not in os.environ\n'
                'p = pathlib.Path(os.environ["BUILDBUDDY_ARTIFACTS_DIRECTORY"])\n'
                'p.mkdir()\n'
                '(p / "failed-child.log").write_text("clean failure")\n'
                'sys.exit(7)\n')
            env = {'PATH': os.defpath, 'BUILDBUDDY_ARTIFACTS_DIRECTORY': str(artifacts),
                   'BUILDBUDDY_API_KEY': 'fake-runner-key', 'MERGIFY_TOKEN': 'fake-token'}
            with patch.dict(os.environ, env, clear=True), \
                 patch.object(sys, 'argv', [str(script)]):
                self.assertEqual(bazel_gate.runner_entrypoint(lambda: self.fail('parent ran gate')), 7)
            self.assertEqual((artifacts / 'failed-child.log').read_text(), 'clean failure')

    def test_runner_request_preserves_auth_for_the_scan_supervisor(self):
        class Args:
            mode, base, head, branch = 'pr', 'a' * 40, 'b' * 40, 'feature'
            repo, cpu, memory, timeout = 'https://github.com/RobTand/trustladderiii', 8, '16GB', 60
        command = remote_gate.run_request(Args)['steps'][-1]['run']
        self.assertIn('child_env.py uv python find 3.14', command)
        self.assertIn('"$scanner_python" tools/ci/bazel_gate.py', command)
        self.assertNotIn('child_env.py python3 tools/ci/bazel_gate.py', command)

    def test_final_scan_marker_defers_without_starting_a_scanner(self):
        output = io.StringIO()
        with patch.dict(os.environ, {'BBRUN_FINAL_SCAN': '1'}, clear=True), \
             patch.object(bazel_gate.subprocess, 'run') as run, \
             contextlib.redirect_stdout(output):
            self.assertEqual(bazel_gate.scan_artifacts(Path('/unused-artifacts')), 0)
        run.assert_not_called()
        self.assertIn('secret scan deferred to bbrun final scan', output.getvalue())

    def test_runner_scans_the_whole_directory_and_deletes_on_failure(self):
        for planted, missing in ((False, False), (True, False), (False, True)):
            with self.subTest(planted=planted, missing=missing), \
                 tempfile.TemporaryDirectory() as directory:
                artifacts = Path(directory, 'runner-artifacts')
                artifacts.mkdir()
                (artifacts / 'unrelated.log').write_text(
                    'fake-runner-secret-value' if planted else 'clean')
                env = {'BUILDBUDDY_API_KEY': 'fake-runner-secret-value',
                       'MERGIFY_TOKEN': 'fake-must-not-reach-scanner', 'PATH': os.defpath}
                if missing:
                    env.pop('BUILDBUDDY_API_KEY')
                real_run = subprocess.run
                calls = []

                def scan(command, **kwargs):
                    calls.append(command)
                    self.assertNotIn('fake-runner-secret-value', command)
                    self.assertEqual(kwargs['env'].get('BUILDBUDDY_API_KEY'),
                                     env.get('BUILDBUDDY_API_KEY'))
                    self.assertNotIn('MERGIFY_TOKEN', kwargs['env'])
                    self.assertEqual(command[-3:], ['--require', 'BUILDBUDDY_API_KEY', str(artifacts)])
                    return real_run(command, **kwargs)

                with patch.dict(os.environ, env, clear=True), \
                     patch.object(bazel_gate.subprocess, 'run', side_effect=scan):
                    status = bazel_gate.scan_artifacts(artifacts)
                self.assertEqual(status, 75 if missing else int(planted))
                self.assertEqual(len(calls), 1)
                self.assertEqual(list(artifacts.iterdir()) == [], planted or missing)

    def test_runner_deletes_existing_artifacts_if_scanner_cannot_start(self):
        with tempfile.TemporaryDirectory() as directory:
            artifacts = Path(directory)
            (artifacts / 'old.log').write_text('old artifact')
            with patch.dict(os.environ, {}, clear=True), \
                 patch.object(bazel_gate.subprocess, 'run', side_effect=OSError):
                with self.assertRaises(OSError):
                    bazel_gate.scan_artifacts(artifacts)
            self.assertEqual(list(artifacts.iterdir()), [])


if __name__ == '__main__':
    unittest.main()
