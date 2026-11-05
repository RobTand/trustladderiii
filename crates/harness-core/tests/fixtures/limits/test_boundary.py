"""#835 boundary parity and the deliberately unchanged inferred-receipt gap."""
import json
from pathlib import Path
import tempfile
import unittest

from tl3_harness.forcing_ownership import valid_delay_budget
from tl3_harness.limits import validate_check_plan
from tl3_harness.model import Check, Kind, Proof
from tl3_harness.proof import assemble_proof


class LimitsBoundary(unittest.TestCase):
    def test_delay_receipts_match_python(self):
        rows = json.loads(Path(__file__).with_name('delay.json').read_text())
        for row in rows:
            with self.subTest(row=row['id']):
                try:
                    value = json.loads(row['json'])
                except ValueError:
                    actual = False
                else:
                    actual = valid_delay_budget(value)
                self.assertEqual(actual, row['valid'])
        print(f'delay boundary: {len(rows)} Python cases executed')

    def test_inferred_receipt_still_passes_admission(self):
        # #835/Q27: a lead must choose receipt admission or external publication.
        # Never truncate inferred identities to make the reservation theorem fit.
        node = 'tests/test_case.py::test_case'
        check = Check('inferred receipt', Kind.DEMONSTRATING, (node,),
                      proof=Proof((), (), 'expected'), declared_symptom={
                          'node': node, 'exception': 'builtins.TimeoutError',
                          'phase': 'call', 'repeat': 1, 'parallel': 0})
        validate_check_plan((check,))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            base, head = root / 'base', root / 'head'
            base.mkdir()
            head.mkdir()
            for index in range(6000):
                name = f'subject_{index:04}_' + 'x' * 100 + '.py'
                (base / name).write_text('value = 0\n')
                (head / name).write_text('value = 1\n')
            receipt = assemble_proof(base, head, root / 'assembled', check, {})
            retained = len(json.dumps(receipt).encode())
            self.assertEqual(len(receipt['subjects']), 6000)
            self.assertGreater(retained, 1_500_000)
            self.assertGreater(retained, 65536 + len(json.dumps(check.proof.as_json())))
            validate_check_plan((check,))
            print(f'inferred gap preserved: admitted 6000 subjects, {retained} retained bytes')


if __name__ == '__main__':
    unittest.main()
