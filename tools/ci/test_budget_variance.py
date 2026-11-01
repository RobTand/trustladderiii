import unittest

import budget_variance


class VarianceTests(unittest.TestCase):
    def test_calibration_uses_run_maxima_so_fast_tests_cannot_dilute_noise(self):
        reports = []
        for run in range(12):
            for index, runner in [(0, 'hosted-arm64'), (1, 'hosted-x64')]:
                elapsed = (10.2289832633 if index == 0 else 11.251069221) if run == 0 else 5
                reports.append({'run_id': run, 'runner_class': runner,
                                'report': {'shard': [index, 2],
                                           'tests': {'noisy': elapsed, **{f'fast-{i}': .001 for i in range(100)}}}})
        # Keep partial-run observations for audit without treating missing shards as coverage.
        reports.append({'run_id': 13, 'runner_class': 'hosted-arm64',
                        'report': {'shard': [0, 4], 'tests': {'noisy': .1}}})
        result = budget_variance.summarize(reports, {'seconds': 10, 'slow': {}})
        for runner, factor in [('hosted-x64', 1.25), ('hosted-arm64', 1.15)]:
            self.assertEqual(result[runner]['multiplier'], factor)
            self.assertEqual(result[runner]['complete_runs'], 12)
        self.assertEqual(result['hosted-arm64']['partial_runs'], 1)
        self.assertGreater(result['hosted-x64']['run_max_ratio_p99'], 1.12)
        self.assertLess(result['hosted-x64']['all_ratios_p99'], 1)


if __name__ == '__main__':
    unittest.main()
