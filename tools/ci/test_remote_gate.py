"""The GitHub side must fail on every way a remote run can go wrong."""
import io
import json
import os
from pathlib import Path
import shlex
import signal
import tempfile
import unittest
from unittest.mock import patch

import remote_gate

HEAD = 'b' * 40


def result(**fields):
    return {'schema': remote_gate.SCHEMA, 'mode': 'pr', 'base': 'a' * 40, 'head': HEAD,
            'error': None, 'inputs': [], 'scopes': ['//crates/kernel'], 'all_scopes': False,
            'selected': ['//crates/kernel:lib_test_tl3_kernel'],
            'verdicts': {'rust': 'pass', 'python': 'skipped'}, **fields}


class FakeClient:
    def __init__(self, statuses, files=None):
        self.statuses, self.files = list(statuses), files or {}

    def call(self, method, body, timeout=120, raw=False):
        if method == 'GetInvocation':
            status = self.statuses.pop(0)
            return {'invocation': [{'invocationStatus': status, 'success': True,
                                    'artifacts': [{'name': name, 'uri': name} for name in self.files]}]}
        if method == 'GetFile':
            # Over HTTP, BuildBuddy returns the file's raw bytes, not JSON.
            assert raw
            return self.files[body['uri']]
        raise AssertionError(method)


class WaitTests(unittest.TestCase):
    def test_waits_for_the_runs_own_terminal_status(self):
        client = FakeClient([None, 'PARTIAL_INVOCATION_STATUS', remote_gate.DONE])
        sleeps = []
        invocation = remote_gate.wait(client, 'id', deadline=10, clock=lambda: 0, sleep=sleeps.append)
        self.assertEqual(invocation['invocationStatus'], remote_gate.DONE)
        self.assertEqual(len(sleeps), 2)

    def test_disconnect_fails(self):
        with self.assertRaisesRegex(remote_gate.RemoteError, 'disconnected'):
            remote_gate.wait(FakeClient([remote_gate.LOST]), 'id', 10, clock=lambda: 0, sleep=print)

    def test_timeout_fails(self):
        with self.assertRaisesRegex(remote_gate.RemoteError, 'in time'):
            remote_gate.wait(FakeClient(['PARTIAL_INVOCATION_STATUS']), 'id', 10,
                             clock=lambda: 11, sleep=print)


    def test_transient_errors_are_retried_then_fail(self):
        class Flaky:
            def __init__(self, failures):
                self.failures = failures

            def call(self, method, body, timeout=120, raw=False):
                if self.failures:
                    self.failures -= 1
                    raise remote_gate.RemoteError('BuildBuddy GetInvocation failed: 503')
                return {'invocation': [{'invocationStatus': remote_gate.DONE}]}
        invocation = remote_gate.wait(Flaky(2), 'id', 10, clock=lambda: 0, sleep=lambda _: None)
        self.assertEqual(invocation['invocationStatus'], remote_gate.DONE)
        with self.assertRaisesRegex(remote_gate.RemoteError, '503'):
            remote_gate.wait(Flaky(10), 'id', 10, clock=lambda: 0, sleep=lambda _: None)


class CancellationTests(unittest.TestCase):
    def test_cancel_uses_the_service_rpc_with_an_in_process_key(self):
        with patch.object(remote_gate.urllib.request, 'urlopen',
                          return_value=io.BytesIO(b'{}')) as request:
            remote_gate.BuildBuddy('synthetic-fixture').call(
                'CancelExecutions', {'invocation_id': 'recorded-id'}, timeout=2)
        sent = request.call_args.args[0]
        self.assertEqual(sent.full_url,
                         'https://app.buildbuddy.io/rpc/BuildBuddyService/CancelExecutions')
        self.assertEqual(json.loads(sent.data), {'invocation_id': 'recorded-id'})
        self.assertEqual(sent.get_header('X-buildbuddy-api-key'), 'synthetic-fixture')
        self.assertNotIn('synthetic-fixture', sent.full_url + sent.data.decode())

    def test_wait_cancels_on_sigint_and_sigterm_then_exits(self):
        for signum in (signal.SIGINT, signal.SIGTERM):
            with self.subTest(signum=signum):
                client = FakeClient(['PARTIAL_INVOCATION_STATUS'])
                calls = []
                original_call = client.call

                def call(method, body, **kwargs):
                    calls.append((method, body))
                    if method == 'CancelExecutions':
                        return {}
                    return original_call(method, body, **kwargs)

                client.call = call
                original_handler = signal.signal(signum, lambda *_: None)
                self.addCleanup(signal.signal, signum, original_handler)
                previous = signal.getsignal(signum)

                def cancel(_):
                    signal.raise_signal(signum)
                    self.fail('wait continued after cancellation')

                with self.assertRaises(SystemExit) as stopped:
                    remote_gate.wait(client, 'recorded-id', 10, clock=lambda: 0,
                                     sleep=cancel)
                self.assertEqual(stopped.exception.code, 128 + signum)
                self.assertIn(('CancelExecutions', {'invocation_id': 'recorded-id'}), calls)
                self.assertEqual(signal.getsignal(signum), previous)

    def test_run_records_invocation_before_waiting(self):
        with tempfile.TemporaryDirectory() as directory:
            invocation_file = Path(directory) / 'tl3-buildbuddy-invocation'
            client = unittest.mock.Mock()
            client.call.return_value = {'invocationId': 'recorded-id'}

            def wait(*_):
                self.assertTrue(invocation_file.is_file(), 'invocation must survive step termination')
                self.assertEqual(invocation_file.read_text().strip(), 'recorded-id')
                raise remote_gate.RemoteError('stop after checking the record')

            with patch.dict(os.environ, {'RUNNER_TEMP': directory,
                                         'BUILDBUDDY_API_KEY': 'synthetic-fixture'}), \
                 patch('sys.argv', ['remote_gate.py', 'pr', '--base', 'a' * 40,
                                    '--head', HEAD, '--branch', 'feature',
                                    '--output-dir', directory]), \
                 patch.object(remote_gate, 'BuildBuddy', return_value=client), \
                 patch.object(remote_gate, 'wait', side_effect=wait):
                self.assertEqual(remote_gate.main(), 1)

    def test_cancelled_step_cancels_recorded_invocation(self):
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / 'tl3-buildbuddy-invocation').write_text('recorded-id\n')
            client = unittest.mock.Mock()
            with patch.dict(os.environ, {'RUNNER_TEMP': directory,
                                         'BUILDBUDDY_API_KEY': 'synthetic-fixture'}), \
                 patch('sys.argv', ['remote_gate.py', 'cancel']), \
                 patch.object(remote_gate, 'BuildBuddy', return_value=client):
                try:
                    code = remote_gate.main()
                except SystemExit as error:
                    self.fail(f'cancel command is unavailable: {error}')
                self.assertEqual(code, 0)
            client.call.assert_called_once_with('CancelExecutions',
                                                {'invocation_id': 'recorded-id'}, timeout=5)

    def test_cancelled_before_submission_needs_no_key_or_api(self):
        with tempfile.TemporaryDirectory() as directory, \
             patch.dict(os.environ, {'RUNNER_TEMP': directory}), \
             patch('sys.argv', ['remote_gate.py', 'cancel']), \
             patch.object(remote_gate, 'BuildBuddy') as client:
            try:
                code = remote_gate.main()
            except SystemExit as error:
                self.fail(f'cancel command is unavailable: {error}')
            self.assertEqual(code, 0)
        client.assert_not_called()


class ResultTests(unittest.TestCase):
    def test_batch_abort_keeps_scopes_and_fails_both_checks(self):
        values = remote_gate.outputs(result(mode='batch-draft',
                                           aborted_early={'label': '//a:failed', 'reason': 'FAILED'}))
        self.assertEqual(values['scopes_ready'], 'true')
        self.assertEqual(values['rust'], 'failure')
        self.assertEqual(values['python'], 'failure')


    def test_merged_pr_lookup_keeps_its_token_out_of_git(self):
        with patch.dict(os.environ, {'BEFORE': 'base', 'AFTER': 'head',
                                    'GITHUB_REPOSITORY': 'owner/repo',
                                    'GH_TOKEN': 'synthetic-fixture'}), \
             patch.object(remote_gate.subprocess, 'check_output', return_value='sha1\nsha2\n') as git, \
             patch.object(remote_gate.urllib.request, 'urlopen', side_effect=[
                 io.BytesIO(b'[{"number": 12}, {"number": 2}]'),
                 io.BytesIO(b'[{"number": 12}]')]) as request, \
             patch.object(remote_gate, 'write_github') as write:
            remote_gate.merged_prs()
        self.assertFalse('GH_TOKEN' in git.call_args.kwargs['env'])
        self.assertEqual(request.call_args.args[0].get_header('Authorization'),
                         'Bearer synthetic-fixture')
        write.assert_called_once_with('GITHUB_OUTPUT', 'prs=2,12\n')

    def test_budget_warnings_reach_job_summary_and_post_merge_record(self):
        warning = {'label': '//crates/store:test_test_postgres', 'test': 'slow',
                   'measured': 56, 'limit': 30, 'metric': 'wall'}
        for mode in ['pr', 'main']:
            with self.subTest(mode=mode):
                receipt = result(mode=mode, budget_warnings=[warning])
                summary = remote_gate.explain(receipt, [12, 13])
                self.assertIn('WARNING', summary)
                for value in [warning['label'], '56', '30']:
                    self.assertIn(value, summary)
                self.assertEqual(remote_gate.post_merge_record(receipt, [12, 13], 1)['budget_warnings'],
                                 [warning])
                if mode == 'pr':
                    self.assertEqual(remote_gate.outputs(receipt)['rust'], 'success')

    def test_result_is_read_from_the_artifact(self):
        client = FakeClient([remote_gate.DONE], {remote_gate.RESULT: json.dumps(result()).encode()})
        invocation = remote_gate.wait(client, 'id', 10, clock=lambda: 0, sleep=print)
        parsed = remote_gate.parse_result(remote_gate.fetch(client, invocation, remote_gate.RESULT), HEAD)
        self.assertEqual(parsed['scopes'], ['//crates/kernel'])

    def test_every_diagnostic_log_artifact_is_saved(self):
        files = {name: b'x' for name in ['pr-bazel-remote-grpc.log', 'main-arm64-execution_log.binpb.zst',
                                          'bazel-remote-grpc.log', remote_gate.RESULT]}
        invocation = remote_gate.wait(FakeClient([remote_gate.DONE], files), 'id', 10, clock=lambda: 0,
                                      sleep=print)
        names = [name for name, _ in remote_gate.diagnostics(FakeClient([], files), invocation)]
        self.assertEqual(sorted(names), ['bazel-remote-grpc.log', 'main-arm64-execution_log.binpb.zst',
                                         'pr-bazel-remote-grpc.log'])

    def test_missing_malformed_or_foreign_results_fail(self):
        for raw in [None, b'not json', json.dumps(result(head='c' * 40)).encode(),
                    json.dumps({**result(), 'schema': 'other'}).encode()]:
            with self.subTest(raw=raw), self.assertRaises(remote_gate.RemoteError):
                remote_gate.parse_result(raw, HEAD)

    def test_outputs_give_scopes_and_check_verdicts(self):
        self.assertEqual(remote_gate.outputs(result()), {
            'scopes_ready': 'true', 'scopes': '//crates/kernel', 'all_scopes': 'false',
            'rust': 'success', 'python': 'success'})

    def test_detection_error_withholds_scopes_and_fails_both_checks(self):
        values = remote_gate.outputs(result(error='bazel-diff exited 1'))
        self.assertEqual(values['scopes_ready'], 'false')
        self.assertEqual((values['rust'], values['python']), ('failure', 'failure'))

    def test_stale_inputs_fail_both_checks(self):
        values = remote_gate.outputs(result(inputs=['bazel run //:gazelle -- -mode=diff']))
        self.assertEqual((values['rust'], values['python']), ('failure', 'failure'))

    def test_run_request_targets_the_head_commit_on_a_pinned_image(self):
        class Args:
            mode, base, head, branch = 'pr', 'a' * 40, HEAD, 'feature'
            repo, cpu, memory, timeout = 'https://github.com/RobTand/trustladderiii', 8, '16GB', 60
        request = remote_gate.run_request(Args)
        self.assertEqual(request['commit_sha'], HEAD)
        self.assertIn('@sha256:', request['platform_properties']['container-image'])
        self.assertIn(f'--base {"a" * 40}', request['steps'][-1]['run'])


    def test_run_request_installs_a_checksummed_uv_before_the_gate(self):
        # The runner image has no uv, and the gate's requirements check runs `uv export`.
        class Args:
            mode, base, head, branch = 'pr', 'a' * 40, HEAD, 'feature'
            repo, cpu, memory, timeout = 'https://github.com/RobTand/trustladderiii', 8, '16GB', 60
        install, gate = [step['run'] for step in remote_gate.run_request(Args)['steps']]
        self.assertIn(f'releases/download/{remote_gate.UV_VERSION}/uv-x86_64-unknown-linux-gnu.tar.gz', install)
        self.assertIn(f'{remote_gate.UV_SHA256}  uv.tar.gz', install)
        self.assertIn('sha256sum -c', install)
        # uv export needs an interpreter matching workers/.python-version (3.14+gil);
        # the image only has 3.12, and uv cannot download a request spelled +gil.
        self.assertIn('uv python install 3.14', install)
        self.assertTrue(gate.startswith('export PATH="$HOME/.local/bin:$PATH" && '))
        self.assertIn('tools/ci/bazel_gate.py', gate)

    def test_run_request_quotes_the_branch_and_requires_both_commits(self):
        class Args:
            mode, base, head, branch = 'pr', 'a' * 40, HEAD, 'feature;touch pwned'
            repo, cpu, memory, timeout = 'https://github.com/RobTand/trustladderiii', 8, '16GB', 60
        words = shlex.split(remote_gate.run_request(Args)['steps'][-1]['run'].split('&& ', 1)[1])
        self.assertEqual(words[words.index('--branch') + 1], 'feature;touch pwned')
        for field in ['head', 'base']:
            with self.subTest(field=field):
                setattr(Args, field, '')
                with self.assertRaises(remote_gate.RemoteError):
                    remote_gate.run_request(Args)
                setattr(Args, field, HEAD if field == 'head' else 'a' * 40)


class WorkflowTests(unittest.TestCase):
    workflow = (Path(__file__).resolve().parents[2] / '.github/workflows/gate.yml').read_text()

    def test_cancellation_cleanup_covers_prs_and_mergify_queue_drafts(self):
        bazel = self.workflow.split('  bazel:', 1)[1].split('  rust-lint:', 1)[0]
        self.assertIn("if: github.event_name == 'pull_request'", bazel)
        self.assertNotIn('github.event.pull_request.draft', bazel)
        self.assertIn('- name: Cancel BuildBuddy run\n        if: cancelled()\n', bazel)
        cleanup = bazel.split('- name: Cancel BuildBuddy run', 1)[1].split('      - ', 1)[0]
        self.assertIn('BUILDBUDDY_API_KEY: ${{ secrets.BUILDBUDDY_API_KEY }}', cleanup)
        self.assertIn('run: python3 tools/ci/remote_gate.py cancel', cleanup)
        self.assertLess(bazel.index('remote_gate.py cancel'), bazel.index('- id: token'))

    def test_an_empty_mergify_token_fails_before_the_upload_is_skipped(self):
        # gha-mergify-ci skips scopes-upload and only warns when its token is empty.
        check = self.workflow.index('test -n "$MERGIFY_TOKEN"')
        self.assertLess(check, self.workflow.index('action: scopes-upload'))

    def test_post_merge_pr_lookup_fails_when_the_api_fails(self):
        self.assertIn('run: python3 tools/ci/remote_gate.py merged-prs', self.workflow)
        with patch.dict(os.environ, {'BEFORE': 'base', 'AFTER': 'head',
                                    'GITHUB_REPOSITORY': 'owner/repo',
                                    'GH_TOKEN': 'synthetic-fixture'}), \
             patch.object(remote_gate.subprocess, 'check_output', return_value='sha1\n'), \
             patch.object(remote_gate.urllib.request, 'urlopen',
                          side_effect=remote_gate.urllib.error.URLError('unavailable')), \
             patch.object(remote_gate, 'write_github') as write:
            with self.assertRaises(remote_gate.urllib.error.URLError):
                remote_gate.merged_prs()
        write.assert_not_called()


class AggregateTests(unittest.TestCase):
    def needs(self, rust='success', uploaded='true', result='success', lint='success'):
        return {'bazel': {'result': result, 'outputs': {'rust': rust, 'python': 'success',
                                                        'scopes_uploaded': uploaded}},
                'rust-lint': {'result': lint}}

    def test_language_passes_only_with_its_verdict_scopes_and_prerequisites(self):
        remote_gate.aggregate('rust', self.needs())
        remote_gate.aggregate('python', self.needs(rust='failure', result='failure'))
        for needs in [self.needs(rust='failure', result='failure'), self.needs(uploaded='false'),
                      self.needs(result='cancelled'), self.needs(lint='failure'), {}]:
            with self.subTest(needs=needs), self.assertRaises(RuntimeError):
                remote_gate.aggregate('rust', needs)


class PostMergeTests(unittest.TestCase):
    def main_result(self, amd64_failed=(), missed=(), arm64_failed=()):
        def arch(failed, missed):
            return {'failed': {'rust': list(failed), 'python': []}, 'missed': list(missed),
                    'verdicts': {}, 'seconds': 1}
        return {**result(mode='main'), 'tests': 3,
                'architectures': {'amd64': arch(amd64_failed, missed), 'arm64': arch(arm64_failed, [])}}

    def test_record_names_prs_and_verdict(self):
        record = remote_gate.post_merge_record(self.main_result(), [301, 302], 120)
        self.assertEqual((record['merge_sha'], record['prs'], record['verdict']), (HEAD, [301, 302], 'pass'))
        self.assertEqual(json.loads(json.dumps(record)), record)

    def test_unselected_failure_is_a_detection_miss(self):
        record = remote_gate.post_merge_record(self.main_result(['//x:t'], ['//x:t']), [7], 1)
        self.assertEqual(record['verdict'], 'detection-miss')
        self.assertIn('detection miss', remote_gate.explain(self.main_result(['//x:t'], ['//x:t']), [7]))
        self.assertIn('#7', remote_gate.explain(self.main_result(['//x:t'], ['//x:t']), [7]))

    def test_selected_failure_and_arm64_failure_are_distinguished(self):
        self.assertEqual(remote_gate.post_merge_record(self.main_result(['//x:t']), [], 1)['verdict'],
                         'selected-but-failed')
        self.assertEqual(remote_gate.post_merge_record(self.main_result(arm64_failed=['//x:t']), [], 1)['verdict'],
                         'arm64-only')


if __name__ == '__main__':
    unittest.main()
