"""The Bazel adapter must use the same exported edges that select tests."""
import unittest

import bazel_graph_impact as adapter


class BazelGraphTests(unittest.TestCase):
    def test_comparison_inputs_keep_package_ceiling_at_base(self):
        base = adapter.from_deps(
            {'//crates/controller:existing': ['//pkg:a.py', '//pkg:b.py'],
             '//tools/tests:small': ['//pkg:a.py'], '//pkg:a.py': [], '//pkg:b.py': []},
            ['//crates/controller:existing', '//tools/tests:small'], ['//pkg:a.py', '//pkg:b.py'])
        head = adapter.from_deps(
            {'//crates/controller:new': ['//pkg:a.py', '//pkg:b.py', '//pkg:c.py'],
             '//new/package:test': ['//pkg:a.py'],
             '//pkg:a.py': [], '//pkg:b.py': [], '//pkg:c.py': []},
            ['//crates/controller:new', '//new/package:test'],
            ['//pkg:a.py', '//pkg:b.py', '//pkg:c.py'])
        comparison = adapter.comparison_inputs(base, head)
        self.assertEqual(comparison['test_packages']['//crates/controller:new'], '//crates/controller')
        self.assertEqual(comparison['test_packages']['//new/package:test'], '//new/package')
        self.assertEqual(comparison['base_package_max_reach'],
                         {'//crates/controller': 2, '//tools/tests': 1})
        self.assertNotIn('//new/package', comparison['base_package_max_reach'])

    def test_edge_diff_uses_each_revisions_types_and_covers_removals(self):
        base = {'reach': {'//test:t': ['//pkg:a.py']},
                'edges': [['//test:t', '//pkg:lib'], ['//pkg:lib', '//pkg:a.py']],
                'node_types': {'//test:t': 'Rule', '//pkg:lib': 'Rule', '//pkg:a.py': 'SourceFile'}}
        head = {'reach': {'//test:t': ['//pkg:b.py']},
                'edges': [['//test:t', '//pkg:new'], ['//pkg:new', '//pkg:b.py']],
                'node_types': {'//test:t': 'Rule', '//pkg:new': 'Rule', '//pkg:b.py': 'SourceFile'}}
        comparison = adapter.comparison_inputs(base, head)
        self.assertEqual(comparison['edge_diff'], {
            'added': {'rule_to_rule': [['//test:t', '//pkg:new']],
                      'rule_to_source': [['//pkg:new', '//pkg:b.py']]},
            'removed': {'rule_to_rule': [['//test:t', '//pkg:lib']],
                        'rule_to_source': [['//pkg:lib', '//pkg:a.py']]},
        })

    def test_leaf_keys_do_not_prove_source_edges_are_present(self):
        deps = {'//pkg:lib': [], '//pkg:src/lib.rs': []}
        self.assertTrue(adapter.needs_source_edges(deps, {'//pkg:src/lib.rs'}))
        stream = '\n'.join([
            '{"type":"RULE","rule":{"name":"//pkg:lib","ruleInput":["//pkg:src/lib.rs"]}}',
            '{"type":"SOURCE_FILE","sourceFile":{"name":"//pkg:src/lib.rs"}}',
        ])
        completed = adapter.add_source_edges(deps, stream)
        self.assertEqual(completed['//pkg:lib'], ['//pkg:src/lib.rs'])
        self.assertFalse(adapter.needs_source_edges(completed, {'//pkg:src/lib.rs'}))

    def test_typed_source_labels_are_taken_from_hash_export(self):
        hashes = {'hashes': {'@@//pkg:lib': {'type': 'Rule', 'hash': 'a'},
                             '@@//pkg:src/lib.rs': {'type': 'SourceFile', 'hash': 'b'},
                             '@@//pkg:tests/t.rs': {'type': 'SourceFile', 'hash': 'c'},
                             '@@other//pkg:src/lib.rs': {'type': 'SourceFile', 'hash': 'd'}}}
        self.assertEqual(adapter.source_labels(hashes), {'//pkg:src/lib.rs'})

    def test_rust_crate_fanout_includes_compile_data_consumers(self):
        deps = {'//crates/a:tl3_a': ['//crates/a:sources'],
                '//crates/a:sources': ['//crates/a:src/lib.rs'],
                '//crates/b:tl3_b': ['//crates/a:tl3_a', '//crates/b:sources'],
                '//crates/b:sources': ['//crates/b:src/lib.rs'],
                '//crates/b:test_t': ['//crates/b:tl3_b'],
                '//ci:test_data': ['//crates/a:sources']}
        result = adapter.crate_report(deps, ['//crates/b:test_t', '//ci:test_data'],
                                      ['//crates/a:tl3_a', '//crates/b:tl3_b'])
        self.assertEqual(result['fanout'], {'//crates/a:tl3_a': 2, '//crates/b:tl3_b': 1})
        self.assertEqual(result['largest_cycle'], [])

    def test_impacted_parser_handles_distances_and_legacy_lines(self):
        self.assertEqual(adapter.impacted_labels('[{"label":"@@//pkg:t","targetDistance":2}]'),
                         {'//pkg:t'})
        self.assertEqual(adapter.impacted_labels('//pkg:t\n'), {'//pkg:t'})
        with self.assertRaises(ValueError):
            adapter.impacted_labels('[{"targetDistance":2}]')


if __name__ == '__main__':
    unittest.main()
