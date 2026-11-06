"""Offline cancel contract tests. Every request goes through the real HTTP client."""
import contextlib
import copy
import http.client
import importlib.util
import io
import json
from pathlib import Path
import unittest
from unittest.mock import patch
import urllib.error

spec = importlib.util.spec_from_file_location('bbrun', Path(__file__).with_name('bbrun.py'))
bbrun = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bbrun)
IID = '10000000-0000-4000-8000-000000000001'
SHA = 'a' * 40
REPO = 'https://github.com/RobTand/trustladderiii'
KEY = 'offline-only-cancel-sentinel'


class ReadFailure:
    def __init__(self, error):
        self.error = error

    def __enter__(self):
        return self

    def __exit__(self, *args):
        return False

    def read(self):
        raise self.error


def invocation(**fields):
    return {'invocation': [{'id': {'invocationId': IID}, 'commitSha': SHA,
                            'repoUrl': REPO, 'invocationStatus': 'PARTIAL_INVOCATION_STATUS', **fields}]}


def execution(**fields):
    return {'execution': [{'executionId': 'offline-execution', 'stage': 'EXECUTING', **fields}]}


def observed(inv=None, exe=None):
    return [('GetInvocation', inv or invocation()), ('GetExecution', exe or execution())]


class CancelTests(unittest.TestCase):
    def run_cli(self, replies, argv=None):
        queue = copy.deepcopy(replies)
        calls = []
        def opened(request, timeout):
            method = request.full_url.rsplit('/', 1)[1]
            calls.append((method, json.loads(request.data)))
            self.assertLessEqual(timeout, 15)
            self.assertEqual(request.get_header('X-buildbuddy-api-key'), KEY)
            self.assertNotIn(KEY.encode(), request.data)
            endpoint = ('https://app.buildbuddy.io/rpc/BuildBuddyService/'
                        if method in ('GetExecution', 'CancelExecutions') else bbrun.API)
            self.assertEqual(request.full_url, endpoint + method)
            self.assertTrue(queue, 'unexpected HTTP request: ' + method)
            expected, response = queue.pop(0)
            self.assertEqual(method, expected)
            if method == 'GetInvocation':
                self.assertEqual(calls[-1][1], {'selector': {'invocation_id': IID}})
            elif method == 'GetExecution':
                self.assertEqual(calls[-1][1], {'execution_lookup': {'invocation_id': IID}})
            else:
                self.assertEqual(method, 'CancelExecutions')
                self.assertEqual(calls[-1][1], {'invocation_id': IID})
            if isinstance(response, Exception):
                raise response
            if isinstance(response, ReadFailure):
                return response
            return io.BytesIO(response if isinstance(response, bytes) else json.dumps(response).encode())
        out, err = io.StringIO(), io.StringIO()
        with patch.object(Path, 'read_text', return_value=KEY) as key_read, \
                patch('urllib.request.urlopen', side_effect=opened), \
                patch.object(bbrun, 'wait_for_gate_window', side_effect=AssertionError('submission gate used')), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            try:
                code = bbrun.main(argv if argv is not None else
                                  ['cancel', IID, '--expect-commit', SHA, '--expect-repo', REPO])
            except SystemExit as error:
                code = error.code
        self.assertNotIn(KEY, out.getvalue() + err.getvalue())
        self.assertFalse(queue, 'missing expected requests')
        return code, out.getvalue(), err.getvalue(), calls, key_read.call_count

    def test_matching_live_case(self):
        replies = observed() + observed() + [('CancelExecutions', {})] + observed(
            exe=execution(stage='COMPLETED', status={'code': 1}))
        code, out, _, calls, _ = self.run_cli(replies)
        self.assertEqual(code, 0)
        self.assertEqual([json.loads(line)['phase'] for line in out.splitlines()],
                         ['before', 'ack', 'after', 'terminal'])
        self.assertIn('cancelled', out)
        self.assertEqual(sum(m == 'CancelExecutions' for m, _ in calls), 1)

    def test_ack_without_terminal_is_unconfirmed(self):
        code, out, _, calls, _ = self.run_cli(observed() + observed() + [('CancelExecutions', {})] + observed())
        self.assertEqual(code, 2)
        self.assertIn('unconfirmed', out)
        self.assertEqual(len(calls), 7)

    def test_argument_guards_before_credentials_or_http(self):
        for argv in (['cancel'], ['cancel', 'short'], ['cancel', IID],
                     ['cancel', IID, '--expect-commit', 'a', '--expect-repo', REPO],
                     ['cancel', IID, '--expect-commit', SHA, '--expect-repo', 'not-url'],
                     ['cancel', IID, IID, '--expect-commit', SHA, '--expect-repo', REPO],
                     ['cancel', IID, '--expect-commit', SHA, '--expect-commit', SHA, '--expect-repo', REPO],
                     ['cancel', IID, '--expect-commit', SHA, '--expect-repo', REPO + '?x=1'],
                     ['cancel', IID.upper().replace('1000', 'ABCD', 1), '--expect-commit', SHA, '--expect-repo', REPO]):
            with self.subTest(argv=argv):
                code, _, _, calls, reads = self.run_cli([], argv)
                self.assertNotEqual(code, 0)
                self.assertEqual((calls, reads), ([], 0))

    def test_protected_before_credentials(self):
        for iid in ('631c29e5-de93-4bde-be66-821decdbc83b', IID):
            argv = ['cancel', iid, '--expect-commit', SHA, '--expect-repo', REPO]
            if iid == IID:
                argv += ['--protect-invocation', IID]
            code, _, _, calls, reads = self.run_cli([], argv)
            self.assertNotEqual(code, 0)
            self.assertEqual((calls, reads), ([], 0))

    def test_source_repo_and_id_mismatch(self):
        for fields in ({'commitSha': 'b'*40}, {'repoUrl': REPO + '.git'},
                       {'id': {'invocationId': '20000000-0000-4000-8000-000000000002'}},
                       {'commit_sha': 'b'*40}, {'repo_url': REPO + '.git'}):
            with self.subTest(fields=fields):
                self.assertEqual(self.run_cli([('GetInvocation', invocation(**fields))])[0], 1)

    def test_malformed_invocations(self):
        for response in ([], None, {}, {'invocation': []}, {'invocation': [None]},
                         {'invocation': invocation()['invocation']*2},
                         invocation(id={}), invocation(invocationStatus='FUTURE_STATUS')):
            with self.subTest(response=response):
                self.assertEqual(self.run_cli([('GetInvocation', response)])[0], 1)

    def test_malformed_ambiguous_executions(self):
        for response in ([], None, {}, {'execution': []}, {'execution': [None]},
                         {'execution': execution()['execution']*2}, execution(executionId=''),
                         execution(stage='UNKNOWN'), execution(stage=True),
                         execution(stage='COMPLETED', status=None), execution(invocationId='another'),
                         execution(execution_id='different')):
            with self.subTest(response=response):
                self.assertEqual(self.run_cli(observed()[:1] + [('GetExecution', response)])[0], 1)

    def test_already_terminal_execution_no_mutation(self):
        for status, exit_code, result in (({}, 0, 'already_terminal_success'),
                                          ({'code': 13}, 0, 'already_terminal_failure'),
                                          ({'code': 1}, 0, 'cancelled'),
                                          ({}, 3, 'already_terminal_failure')):
            code, out, _, calls, _ = self.run_cli(observed(exe=execution(
                stage=4, status=status, exitCode=exit_code)))
            self.assertEqual(code, 0)
            self.assertIn(result, out)
            self.assertEqual(len(calls), 2)

    def test_terminal_invocation_no_mutation(self):
        for success, result in ((True, 'already_terminal_success'), (False, 'already_terminal_failure')):
            code, out, _, calls, _ = self.run_cli(observed(inv=invocation(
                invocationStatus='COMPLETE_INVOCATION_STATUS', success=success)))
            self.assertEqual(code, 0)
            self.assertIn(result, out)
            self.assertEqual(len(calls), 2)

    def test_changed_ownership_before_mutation(self):
        for inv, exe in ((invocation(commitSha='b'*40), execution()),
                         (invocation(repoUrl=REPO+'.git'), execution()),
                         (invocation(user='different-owner'), execution()),
                         (invocation(), execution(executionId='new-execution'))):
            replies = observed() + [('GetInvocation', inv)]
            if inv == invocation() or inv == invocation(user='different-owner'):
                replies += [('GetExecution', exe)]
            self.assertEqual(self.run_cli(replies)[0], 1)

    def test_completion_during_recheck_no_mutation(self):
        code, out, _, calls, _ = self.run_cli(observed() + observed(
            exe=execution(stage=4, status={})))
        self.assertEqual(code, 0)
        self.assertIn('already_terminal_success', out)
        self.assertEqual(len(calls), 4)

    def test_secret_in_remote_evidence_or_error_never_printed(self):
        replies = observed(inv=invocation(user=KEY)) + observed(inv=invocation(user=KEY))
        replies += [('CancelExecutions', {'unexpected': KEY})] + observed(
            inv=invocation(user=KEY), exe=execution(stage=4, status={'code': 1, 'message': KEY}))
        self.assertEqual(self.run_cli(replies)[0], 0)
        self.assertEqual(self.run_cli([('GetInvocation', urllib.error.URLError(KEY))])[0], 1)

    def test_rpc_failure_no_mutation_retry(self):
        for prefix, method in (([], 'GetInvocation'), (observed()[:1], 'GetExecution'),
                               (observed()+observed(), 'CancelExecutions')):
            code, out, _, calls, _ = self.run_cli(prefix + [(method, urllib.error.URLError(KEY))])
            self.assertEqual(code, 1 if method != 'CancelExecutions' else 2)
            self.assertLessEqual(sum(m == 'CancelExecutions' for m, _ in calls), 1)
            if method == 'CancelExecutions':
                self.assertIn('unconfirmed', out)

    def test_after_failure_or_changed_identity_is_unconfirmed(self):
        prefix = observed() + observed() + [('CancelExecutions', {})]
        for response in (invocation(commitSha='b'*40), urllib.error.URLError(KEY)):
            code, out, _, _, _ = self.run_cli(prefix + [('GetInvocation', response)])
            self.assertEqual(code, 2)
            self.assertIn('unconfirmed', out)

    def test_response_read_failures_before_mutation_are_refused(self):
        for error in (ConnectionResetError(KEY), http.client.IncompleteRead(KEY.encode(), 100)):
            for prefix, method in (([], 'GetInvocation'), (observed()[:1], 'GetExecution'),
                                   (observed(), 'GetInvocation'),
                                   (observed() + observed()[:1], 'GetExecution')):
                with self.subTest(error=type(error).__name__, reads=len(prefix)):
                    code, out, err, calls, _ = self.run_cli(prefix + [(method, ReadFailure(error))])
                    self.assertEqual(code, 1)
                    self.assertEqual(json.loads(out.splitlines()[-1]), {
                        'phase': 'terminal', 'outcome': 'refused', 'cancellation_attempted': False})
                    self.assertFalse(any(method == 'CancelExecutions' for method, _ in calls))
                    self.assertNotIn('Traceback', err)

    def test_response_read_failures_after_attempt_are_unconfirmed(self):
        for error in (ConnectionResetError(KEY), http.client.IncompleteRead(KEY.encode(), 100)):
            for suffix, method in (([], 'CancelExecutions'),
                                   ([('CancelExecutions', {})], 'GetInvocation'),
                                   ([('CancelExecutions', {})] + observed()[:1], 'GetExecution')):
                with self.subTest(error=type(error).__name__, reads=len(suffix)):
                    code, out, err, calls, _ = self.run_cli(
                        observed() + observed() + suffix + [(method, ReadFailure(error))])
                    self.assertEqual(code, 2)
                    self.assertEqual(json.loads(out.splitlines()[-1]), {
                        'phase': 'terminal', 'outcome': 'unconfirmed', 'cancellation_attempted': True})
                    self.assertEqual(sum(method == 'CancelExecutions' for method, _ in calls), 1)
                    self.assertIn('do not blindly retry', err)
                    self.assertNotIn('Traceback', err)

    def test_duplicate_json_fields_are_ambiguous(self):
        data = json.dumps(invocation()).encode()
        data = data.replace(b'"commitSha":', b'"commitSha": "wrong", "commitSha":')
        self.assertEqual(self.run_cli([('GetInvocation', data)])[0], 1)

    def test_malformed_ack_is_not_completion(self):
        code, out, _, calls, _ = self.run_cli(observed()+observed()+[('CancelExecutions', b'{bad')])
        self.assertEqual(code, 2)
        self.assertIn('unconfirmed', out)
        self.assertEqual(len(calls), 5)


if __name__ == '__main__':
    unittest.main(verbosity=2)
