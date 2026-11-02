"""Remote executor budget classes come from repeated, uncached Bazel runs."""
import io
import json
from pathlib import Path
import tempfile
import unittest
import zipfile

import bazel_budget_calibration as calibration

POLICY = {'seconds': 10, 'slow': {'slow_test': 20}}


def receipt(directory, target, run, total, seconds, runner='baseline', zipped=False):
    """Bazel 9 leaves test.outputs/ unzipped by default; older layouts zip it."""
    folder = Path(directory, target, f'run_{run}_of_{total}', 'test.outputs')
    folder.mkdir(parents=True)
    body = json.dumps({'rust': {'b': list(seconds)},
                       'seconds': {f'b::{name}': value for name, value in seconds.items()},
                       'runner_class': runner})
    if zipped:
        data = io.BytesIO()
        with zipfile.ZipFile(data, 'w') as archive:
            archive.writestr('inventory.json', body)
        (folder / 'outputs.zip').write_bytes(data.getvalue())
    else:
        (folder / 'inventory.json').write_text(body)


class CalibrationTests(unittest.TestCase):
    def test_cpu_calibration_tightens_only_observed_existing_limits(self):
        policy = {'seconds': 10, 'slow': {'b::slow': 20, 'b::unmeasured': 30}}
        with tempfile.TemporaryDirectory() as directory:
            paths = []
            for index, wall in enumerate([8, 60]):
                path = Path(directory, f'{index}.json')
                path.write_text(json.dumps({'seconds': {'b::slow': wall},
                                            'cpu_seconds': {'b::slow': 1.2 + index * .1}}))
                paths.append(path)
            summary = calibration.calibrate_cpu(paths, policy)
            self.assertEqual(summary['cpu'], {'seconds': 10,
                                              'slow': {'b::slow': 3, 'b::unmeasured': 30}})
            self.assertEqual(len(summary['receipts']), 2)

    def test_cpu_calibration_refuses_receipts_without_cpu_or_a_second_run(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory, 'receipt.json')
            path.write_text(json.dumps({'seconds': {'b::slow': 3}}))
            with self.assertRaisesRegex(ValueError, 'two'):
                calibration.calibrate_cpu([path], POLICY)
            with self.assertRaisesRegex(ValueError, 'CPU'):
                calibration.calibrate_cpu([path, path], POLICY)

    def test_multiplier_follows_the_hosted_method(self):
        with tempfile.TemporaryDirectory() as directory:
            for run in range(1, 11):
                receipt(directory, 'crates/a', run, 10, {'fast': 9 + run * 0.2, 'slow_test': 10})
            summary = calibration.summarize(Path(directory), POLICY, 'buildbuddy-amd64')
        # The worst run reaches 11/10; *1.10 = 1.21, rounded up to the next 0.05.
        self.assertEqual(summary['complete_runs'], 10)
        self.assertAlmostEqual(summary['run_max_ratio_p99'], 1.1)
        self.assertEqual(summary['multiplier'], 1.25)
        self.assertEqual(len(summary['overruns']), 5)

    def test_zipped_receipts_are_read_too(self):
        with tempfile.TemporaryDirectory() as directory:
            for run in range(1, 11):
                receipt(directory, 'crates/a', run, 10, {'fast': 5}, zipped=True)
            self.assertEqual(calibration.summarize(Path(directory), POLICY, 'x')['complete_runs'], 10)

    def test_tail_markers_use_the_default_budget(self):
        with tempfile.TemporaryDirectory() as directory:
            for run in range(1, 11):
                receipt(directory, 'crates/a', run, 10, {'__tail__': 5})
            self.assertEqual(calibration.summarize(Path(directory), POLICY, 'x')['multiplier'], 1.0)

    def test_fewer_than_ten_complete_runs_is_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            for run in range(1, 10):
                receipt(directory, 'crates/a', run, 9, {'fast': 1})
            with self.assertRaisesRegex(ValueError, 'ten complete runs'):
                calibration.summarize(Path(directory), POLICY, 'x')

    def test_a_run_missing_a_binary_is_incomplete(self):
        with tempfile.TemporaryDirectory() as directory:
            for run in range(1, 11):
                receipt(directory, 'crates/a', run, 10, {'fast': 1})
                if run != 3:
                    receipt(directory, 'crates/b', run, 10, {'fast': 1})
            with self.assertRaisesRegex(ValueError, 'ten complete runs'):
                calibration.summarize(Path(directory), POLICY, 'x')


if __name__ == '__main__':
    unittest.main()
