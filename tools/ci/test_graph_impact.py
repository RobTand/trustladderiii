"""History-free graph checks, including planted examples of every failure kind."""
import hashlib
import json
from pathlib import Path
import unittest

import graph_impact as impact


def graph(deps=None, tests=('//tests:small', '//tests:other'), sources=('//pkg:a.py', '//pkg:b.py'),
          loc=None):
    return impact.from_deps(deps or {'//tests:small': ['//pkg:a.py'],
                                    '//tests:other': ['//pkg:b.py']}, tests, sources, loc)


class GraphImpactTests(unittest.TestCase):
    def test_metrics_include_unused_sources_and_loc_weighting(self):
        current = graph(sources=('//pkg:a.py', '//pkg:b.py', '//pkg:unused.py'),
                        loc={'//pkg:a.py': 8, '//pkg:b.py': 1, '//pkg:unused.py': 1})
        measured = impact.metrics(current)
        self.assertAlmostEqual(measured['uniform'], 1 / 3)
        self.assertAlmostEqual(measured['loc_uniform'], .45)
        self.assertEqual(measured['reach'], {'median': 1, 'p90': 1, 'max': 1})
        self.assertEqual(measured['scc_max'], 0)

    def test_closure_handles_diamonds_and_cycles_without_double_counting(self):
        current = graph({'//tests:small': ['//pkg:lib'], '//tests:other': ['//pkg:b.py'],
                         '//pkg:lib': ['//pkg:a.py', '//pkg:hub'],
                         '//pkg:hub': ['//pkg:lib', '//pkg:a.py']})
        self.assertEqual(current.tests['//tests:small'], {'//pkg:a.py'})
        self.assertEqual(impact.metrics(current)['scc_members'], [['//pkg:hub', '//pkg:lib']])

    def test_new_broad_test_fails_and_names_a_hub_and_added_edge(self):
        base = graph()
        head = graph({'//tests:small': ['//pkg:a.py'], '//tests:other': ['//pkg:b.py'],
                      '//tests:new': ['//pkg:hub'], '//pkg:hub': ['//pkg:a.py', '//pkg:b.py']},
                     tests=('//tests:small', '//tests:other', '//tests:new'))
        result = impact.check(base, head, {'//tests:new'}, impact.baseline(base))
        self.assertTrue(any('reach 2 > budget 1' in error for error in result['errors']))
        self.assertIn('//tests:new', result['explain_targets'])
        self.assertIn(['//tests:new', '//pkg:hub'], result['added_edges'])
        self.assertIn('new tests 1', result['summary'])

    def test_changed_broad_test_fails_but_unchanged_legacy_test_does_not(self):
        current = graph({'//tests:small': ['//pkg:a.py', '//pkg:b.py'],
                         '//tests:other': ['//pkg:b.py']})
        policy = impact.baseline(current)
        policy['reach_budget'] = 1
        self.assertFalse(impact.check(current, current, set(), policy)['errors'])
        self.assertTrue(any('reach 2 > budget 1' in error for error in
                            impact.check(current, current, {'//tests:small'}, policy)['errors']))

    def test_new_test_to_test_and_conftest_edges_fail(self):
        base = graph()
        for source, target in [('//tests:small', '//tests:other'),
                               ('//tests:conftest.py', '//pkg:a.py')]:
            head = graph({'//tests:small': ['//pkg:a.py'], '//tests:other': ['//pkg:b.py'],
                          source: [target]})
            result = impact.check(base, head, set(), impact.baseline(base))
            self.assertTrue(any('forbidden edge' in error for error in result['errors']), result)

    def test_uniform_regression_reports_added_edge_and_fanout_growth(self):
        base = graph()
        head = graph({'//tests:small': ['//pkg:a.py', '//pkg:b.py'],
                      '//tests:other': ['//pkg:b.py']})
        result = impact.check(base, head, set(), impact.baseline(base))
        self.assertTrue(any('U worsened' in error for error in result['errors']))
        self.assertEqual(result['fanout_growth'][0], ['//pkg:b.py', 1])
        self.assertIn(['//tests:small', '//pkg:b.py'], result['added_edges'])

    def test_scc_merge_fails_even_when_a_larger_existing_cycle_masks_the_size(self):
        deps = {'//tests:small': ['//pkg:a.py'], '//tests:other': ['//pkg:b.py'],
                '//a:x': ['//a:y'], '//a:y': ['//a:x'],
                '//b:x': ['//b:y'], '//b:y': ['//b:x'],
                '//c:a': ['//c:b'], '//c:b': ['//c:c'], '//c:c': ['//c:d'],
                '//c:d': ['//c:e'], '//c:e': ['//c:a']}
        base = graph(deps)
        head = graph({**deps, '//a:x': ['//a:y', '//b:x'], '//b:y': ['//b:x', '//a:y']})
        result = impact.check(base, head, set(), impact.baseline(base))
        self.assertTrue(any('SCC grew or merged' in error for error in result['errors']))

    def test_missing_recorded_gain_and_baseline_writer_refuses_losses(self):
        base = graph()
        head = graph({'//tests:small': [], '//tests:other': ['//pkg:b.py']})
        policy = impact.baseline(base)
        self.assertTrue(any('record gain' in error for error in
                            impact.check(base, head, set(), policy)['errors']))
        improved = impact.write_baseline(head, policy)
        self.assertEqual(improved['uniform'], .25)
        with self.assertRaisesRegex(ValueError, 'refuses losses'):
            impact.write_baseline(base, improved)
        self.assertFalse(impact.check(base, head, set(), improved)['errors'])

    def test_allowlist_requires_reasons_and_only_shrinks(self):
        base = graph()
        policy = impact.baseline(base)
        with self.assertRaisesRegex(ValueError, 'reason'):
            impact.check(base, base, set(), policy, {'reach://tests:small': ''})
        result = impact.check(base, base, set(), policy, {'reach://tests:small': 'integration'}, {})
        self.assertTrue(any('allowlist may only shrink' in error for error in result['errors']))

    def test_empty_or_incomplete_graph_fails_closed(self):
        with self.assertRaisesRegex(ValueError, 'test.*missing'):
            impact.from_deps({}, ['//tests:missing'], ['//pkg:a.py'])
        with self.assertRaisesRegex(ValueError, 'source'):
            impact.from_deps({'//tests:t': []}, ['//tests:t'], [])

    def test_core_hash_is_recorded(self):
        root = Path(__file__).parent
        record = json.loads((root / 'graph-impact-core.json').read_text())
        self.assertEqual(hashlib.sha256((root / 'graph_impact.py').read_bytes()).hexdigest(),
                         record['sha256'])


if __name__ == '__main__':
    unittest.main()
