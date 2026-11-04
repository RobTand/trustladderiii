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
