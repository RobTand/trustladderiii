"""The Bazel adapter must use the same exported edges that select tests."""
import unittest
import hashlib
import json
from pathlib import Path
import tempfile

import bazel_graph_impact as adapter


class BazelGraphTests(unittest.TestCase):
    def test_load_records_file_content_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'pkg').mkdir()
            content = b'fn contracts() {}\n'
            (root / 'pkg/lib.rs').write_bytes(content)
            deps = root / 'deps.json'
            deps.write_text(json.dumps({'//pkg:test': ['//pkg:lib.rs'], '//pkg:lib.rs': []}))
            hashes = root / 'hashes.json'
            hashes.write_text(json.dumps({'//pkg:test': 'Rule#test', '//pkg:lib.rs': 'SourceFile#file'}))
            measured, _, _ = adapter.load(deps, hashes, ['//pkg:test'], root)
            self.assertEqual(measured.get('source_identities'),
                             {'//pkg:lib.rs': hashlib.sha256(content).hexdigest()})

    def test_load_records_cargo_package_ownership(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'unrelated-directory').mkdir()
            (root / 'unrelated-directory/lib.rs').write_text('fn core() {}\n')
            (root / 'unrelated-directory/Cargo.toml').write_text(
                '[package]\nname = "tl3-controller-core"\nversion = "0.1.0"\n')
            source = '//unrelated-directory:lib.rs'
            deps = root / 'deps.json'
            deps.write_text(json.dumps({'//tests:t': [source], source: []}))
            hashes = root / 'hashes.json'
            hashes.write_text(json.dumps({'//tests:t': 'Rule#test', source: 'SourceFile#file'}))
            measured, _, _ = adapter.load(deps, hashes, ['//tests:t'], root)
            self.assertEqual(measured.get('crate_packages'),
                             {'tl3-controller-core': '//unrelated-directory'})

    def crate_graph(self, count=1, reached=None, other=False):
        test = '//crates/controller:test_controller'
        old = '//crates/controller:src/lib.rs'
        sources = [f'//crates/controller-core:src/core{i}.rs' for i in range(count)]
        deps = {test: [old], old: []}
        if sources:
            deps[test].append('//crates/controller-core:tl3_controller_core')
            deps['//crates/controller-core:tl3_controller_core'] = sources[:reached]
            deps.update({source: [] for source in sources})
        if other:
            sources.append('//crates/unrelated:src/lib.rs')
            deps[test].append(sources[-1])
            deps[sources[-1]] = []
        graph = adapter.from_deps(deps, [test], [old, *sources])
        graph['crate_packages'] = {'tl3-controller-core': '//crates/controller-core'} if count else {}
        return graph

    def crate_entry(self, count=1):
        return {'crate:tl3-controller-core': {'reason': 'Decision crate for #744.', 'sources': count}}

    def check_crate(self, head, entries=None, base=None, previous=None):
        base = self.crate_graph(0) if base is None else base
        return adapter.check(base, head, set(head['reach']), adapter.baseline(base),
                             self.crate_entry() if entries is None else entries,
                             {} if previous is None else previous)

    def test_crate_within_bound_excuses_structural_and_per_test_reach(self):
        result = self.check_crate(self.crate_graph())
        self.assertEqual(result['errors'], [])
        self.assertIn('Added allowlist entry crate:tl3-controller-core', result['summary'])
        self.assertIn('Decision crate for #744.', result['summary'])
        self.assertIn('sources', result['summary'])
        self.assertEqual(len(next(iter(result['head']['reach'].values()))), 2)

    def test_crate_reach_past_bound_fails_with_key_and_bound(self):
        result = self.check_crate(self.crate_graph(2))
        errors = '; '.join(result['errors'])
        self.assertIn('crate:tl3-controller-core', errors)
        self.assertIn('N=1', errors)
        self.assertTrue(any('reach' in error for error in result['errors']), errors)

    def test_crate_does_not_excuse_other_package_or_subpackage_growth(self):
        for outsider in ('//crates/unrelated:src/lib.rs', '//crates/controller-core/nested:lib.rs'):
            with self.subTest(outsider=outsider):
                head = self.crate_graph(other=True)
                head['reach'] = {test: [outsider if 'unrelated' in source else source for source in sources]
                                 for test, sources in head['reach'].items()}
                head['loc'][outsider] = head['loc'].pop('//crates/unrelated:src/lib.rs')
                head['edges'] = [[a, outsider if 'unrelated' in b else b] for a, b in head['edges']]
                head['node_types'][outsider] = 'SourceFile'
                head['dependents'][outsider] = 1
                result = self.check_crate(head)
                self.assertTrue(any('existing test reach grew' in error for error in result['errors']))
                self.assertTrue(any('reach 2 exceeds budget 1' in error for error in result['errors']))
                self.assertTrue(any(outsider in error for error in result['errors']))

    def test_crate_real_count_requires_update_even_when_not_all_sources_reached(self):
        result = self.check_crate(self.crate_graph(2, reached=1))
        errors = '; '.join(result['errors'])
        self.assertIn('crate:tl3-controller-core', errors)
        self.assertIn('N=1', errors)
        self.assertIn('2', errors)
        self.assertIn('update N', errors)

    def test_crate_entry_requires_issue_reason(self):
        for entry in ({'sources': 1}, {'reason': '', 'sources': 1},
                      {'reason': 'Decision crate', 'sources': 1}):
            with self.subTest(entry=entry):
                errors = adapter.allowlist_errors({'crate:tl3-controller-core': entry}, {})
                self.assertTrue(any('reason' in error for error in errors))

    def test_crate_entry_requires_nonnegative_integer_count(self):
        for count in (None, -1, True, 1.5, '1'):
            with self.subTest(count=count):
                errors = adapter.allowlist_errors(self.crate_entry(count), {})
                self.assertTrue(any('sources' in error for error in errors), errors)

    def test_crate_reason_immutable_but_count_tracks_real_changes(self):
        for old_count, new_count in ((1, 2), (2, 1)):
            with self.subTest(old=old_count, new=new_count):
                result = self.check_crate(self.crate_graph(new_count), self.crate_entry(new_count),
                                          self.crate_graph(old_count), self.crate_entry(old_count))
                self.assertEqual(result['errors'], [])
                self.assertIn('Changed allowlist entry crate:tl3-controller-core', result['summary'])
                self.assertIn(str(new_count), result['summary'])
        entries = self.crate_entry()
        entries['crate:tl3-controller-core']['reason'] = 'Different reason for #744.'
        self.assertTrue(any('reason cannot change' in error for error in
                            adapter.allowlist_errors(entries, self.crate_entry())))

    def test_crate_count_cannot_change_without_real_source_count_change(self):
        result = self.check_crate(self.crate_graph(), self.crate_entry(2),
                                  self.crate_graph(), self.crate_entry())
        self.assertTrue(any('update N' in error for error in result['errors']), result['errors'])

    def test_crate_cannot_keep_excess_budget_after_source_deletion(self):
        result = self.check_crate(self.crate_graph(), self.crate_entry(2),
                                  self.crate_graph(2), self.crate_entry(2))
        self.assertTrue(any('update N' in error for error in result['errors']), result['errors'])

    def test_crate_must_resolve_to_a_cargo_package_in_head(self):
        result = self.check_crate(self.crate_graph(0))
        self.assertTrue(any('crate:tl3-controller-core' in error and 'package' in error
                            for error in result['errors']), result['errors'])

    def test_crate_exception_preserves_existing_source_fanout_checks(self):
        base = self.crate_graph()
        head = self.crate_graph()
        other = '//crates/controller:test_other'
        old = '//crates/controller:src/lib.rs'
        source = '//crates/controller-core:src/core0.rs'
        base['reach'][other], head['reach'][other] = [old], [old, source]
        for graph in (base, head):
            graph['edges'].append([other, old])
            graph['test_packages'][other] = '//crates/controller'
            graph['node_types'][other] = 'Rule'
        head['edges'].append([other, source])
        result = self.check_crate(head, base=base, previous=self.crate_entry())
        self.assertTrue(any('existing source fanout grew' in error for error in result['errors']))

    def test_crate_does_not_double_discount_moved_sources(self):
        base, head = self.crate_graph(0), self.crate_graph(other=True)
        old = '//crates/controller:src/lib.rs'
        moved = '//crates/controller-core:src/core0.rs'
        head['reach'] = {test: [source for source in sources if source != old]
                         for test, sources in head['reach'].items()}
        del head['loc'][old]
        head['edges'] = [[a, b] for a, b in head['edges'] if b != old]
        head['source_identities'], base['source_identities'] = {moved: 'identical'}, {old: 'identical'}
        result = self.check_crate(head, base=base)
        self.assertTrue(any('existing test reach grew' in error for error in result['errors']))
        self.assertTrue(any('reach 2 exceeds budget 1' in error for error in result['errors']))

    def test_per_test_reach_string_exception_unchanged(self):
        test = '//crates/controller:test_controller'
        entries = {f'reach:{test}': 'Controller extraction for #744.'}
        result = self.check_crate(self.crate_graph(2, other=True), entries)
        self.assertEqual(result['errors'], [])

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
