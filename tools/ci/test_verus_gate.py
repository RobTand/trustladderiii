"""Verus proof discovery stays separate from Cargo test parity (#770)."""
import json
import unittest

import bazel_parity


def proof(**changes):
    attributes = {'tags': ['rust', 'verus'], 'expected_obligations': 1,
                  'src': '//crates/mergify:proofs/delivery.rs',
                  'srcs': ['//crates/mergify:src/decide/delivery_step.rs'],
                  'specs': [], 'timeout': 'short'} | changes
    return json.dumps({'rule': {
        'name': '//crates/mergify:verus_delivery', 'ruleClass': 'verus_test',
        'attribute': [{'name': key, 'stringListValue': value} if isinstance(value, list)
                      else {'name': key, 'intValue': value} if isinstance(value, int)
                      else {'name': key, 'stringValue': value}
                      for key, value in attributes.items()]}})


class VerusGateTests(unittest.TestCase):
    def test_valid_proof_has_a_separate_inventory(self):
        checker = getattr(bazel_parity, 'verus_tests', None)
        self.assertIsNotNone(checker, 'Gate must discover and validate Verus proofs')
        self.assertEqual(checker([proof()]), {'//crates/mergify:verus_delivery': 1})
        self.assertEqual(bazel_parity.bazel_tests([proof()]), ({}, []))

    def test_vacuous_or_ungated_proofs_are_refused(self):
        checker = getattr(bazel_parity, 'verus_tests', None)
        self.assertIsNotNone(checker, 'Gate must reject vacuous or ungated proofs')
        for changes in [{'expected_obligations': 0}, {'tags': ['verus']},
                        {'tags': ['rust', 'verus', 'manual']}, {'srcs': []}]:
            with self.subTest(changes=changes), self.assertRaises(RuntimeError):
                checker([proof(**changes)])


class RequiredRustCheckTests(unittest.TestCase):
    def test_verus_failure_reaches_the_required_rust_aggregate(self):
        import bazel_gate
        import remote_gate
        label = '//crates/mergify:verus_delivery'
        verdicts, failed = bazel_gate.classify([label], {label: 'FAILED'}, {label})
        self.assertEqual(failed['rust'], [label])
        outputs = remote_gate.outputs({'mode': 'pr', 'verdicts': verdicts})
        self.assertEqual(outputs['rust'], 'failure')
        with self.assertRaisesRegex(RuntimeError, 'failed rust'):
            remote_gate.aggregate('rust', {'bazel': {'result': 'failure',
                'outputs': outputs | {'scopes_uploaded': 'true'}}})

    def test_receipts_require_the_declared_positive_obligation_count(self):
        for expected, verified in [(0, 0), (1, 0), (2, 1)]:
            with self.subTest(expected=expected, verified=verified), self.assertRaises(RuntimeError):
                bazel_parity.check_verus_receipt({'proofs': {'proof': {
                    'kind': 'verus', 'expected_obligations': expected, 'verified': verified}}})
        bazel_parity.check_verus_receipt({'proofs': {'proof': {
            'kind': 'verus', 'expected_obligations': 2, 'verified': 2}}})

    def test_nightly_executes_every_verus_proof_and_fails_on_any_failure(self):
        from types import SimpleNamespace
        import bazel_gate
        label = '//crates/mergify:verus_delivery'
        calls = []

        def run(targets, config, metadata, log_name):
            calls.append((targets, config))
            return {label: 'FAILED'}

        workspace = SimpleNamespace(verus_tests=lambda: {label}, test=run)
        result = bazel_gate.gate_proofs(workspace, {})
        self.assertEqual(calls, [([label], 'remote')])
        self.assertEqual(result['verdicts']['rust'], 'fail')
