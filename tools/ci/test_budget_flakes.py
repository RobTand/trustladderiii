import unittest

import budget_flakes


class FlakeAuditTests(unittest.TestCase):
    def test_distributions_use_nearest_rank_and_each_architecture_limit(self):
        reports = []
        for run in range(20):
            for arch in ('X64', 'ARM64'):
                reports.append({'run_id': run, 'arch': arch,
                                'tests': {'writer': run + 1, 'near': 10.5,
                                          'below': 10.34, 'boundary': 11.25}})
        result = budget_flakes.summarize(reports, {'seconds': 10, 'slow': {'writer': 15}})
        for arch in ('X64', 'ARM64'):
            writer = result[arch]['tests']['writer']
            self.assertEqual(writer['n'], 20)
            self.assertEqual(writer['median'], 10.5)
            self.assertEqual(writer['p95'], 19)
            self.assertEqual(writer['max'], 20)
            self.assertEqual(writer['max_run'], 19)
        self.assertEqual(result['X64']['tests']['writer']['effective_limit'], 18.75)
        self.assertEqual(result['ARM64']['tests']['writer']['effective_limit'], 17.25)
        self.assertEqual(result['X64']['near_limit'], ['boundary', 'writer'])
        self.assertEqual(result['ARM64']['near_limit'], ['boundary', 'near', 'writer'])

    def test_missing_architecture_samples_remain_missing(self):
        result = budget_flakes.summarize([
            {'run_id': 1, 'arch': 'ARM64', 'tests': {'writer': 19.429581394}},
            {'run_id': 1, 'arch': 'X64', 'tests': {'other': 1}},
        ], {'seconds': 10, 'slow': {'writer': 15}})
        self.assertNotIn('writer', result['X64']['tests'])
        self.assertEqual(result['ARM64']['tests']['writer']['n'], 1)
        self.assertEqual(result['ARM64']['tests']['writer']['p95'], 19.429581394)


if __name__ == '__main__':
    unittest.main()
