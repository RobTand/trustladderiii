"""The GitHub side must fail on every way a remote run can go wrong."""
import json
import unittest

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


class ResultTests(unittest.TestCase):
    def test_result_is_read_from_the_artifact(self):
        client = FakeClient([remote_gate.DONE], {remote_gate.RESULT: json.dumps(result()).encode()})
        invocation = remote_gate.wait(client, 'id', 10, clock=lambda: 0, sleep=print)
        parsed = remote_gate.parse_result(remote_gate.fetch(client, invocation, remote_gate.RESULT), HEAD)
        self.assertEqual(parsed['scopes'], ['//crates/kernel'])

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
        self.assertIn(f'--base {"a" * 40}', request['steps'][0]['run'])


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
