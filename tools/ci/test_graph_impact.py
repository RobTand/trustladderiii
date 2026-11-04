"""History-free graph checks, including planted examples of every failure kind."""
import hashlib
import json
from pathlib import Path
import unittest

import bazel_graph_impact as impact


def graph(deps=None, tests=('//tests:small', '//tests:other'), sources=('//pkg:a.py', '//pkg:b.py'),
          loc=None):
    deps = deps or {'//tests:small': ['//pkg:a.py'], '//tests:other': ['//pkg:b.py']}
    return impact.from_deps({**{source: [] for source in sources}, **deps}, tests, sources, loc)


class GraphImpactTests(unittest.TestCase):
    def test_existing_edges_missing_from_stored_baseline_are_not_new(self):
        policy = impact.baseline(graph())
        for consumer, dependency in [('//tests:small', '//tests:other'),
                                     ('//tests:conftest.py', '//pkg:a.py')]:
            with self.subTest(edge=(consumer, dependency)):
                current = graph({'//tests:small': ['//pkg:a.py'],
                                 '//tests:other': ['//pkg:b.py'], consumer: [dependency]})
                result = impact.check(current, current, set(), policy)
                self.assertEqual(result['errors'], [])
                self.assertEqual(result['added_edges'], [])

    def test_stored_edge_exceptions_cannot_hide_a_pr_edge(self):
        base = graph()
        head = graph({'//tests:small': ['//pkg:a.py', '//tests:other'],
                      '//tests:other': ['//pkg:b.py']})
        policy = impact.baseline(head)
        result = impact.check(base, head, set(), policy)
        self.assertIn('new test_to_test edge: //tests:small -> //tests:other', result['errors'])

    def test_baseline_records_and_writes_never_store_edges(self):
        current = graph({'//tests:small': ['//tests:other'], '//tests:other': ['//pkg:b.py']})
        policy = impact.baseline(current)
        self.assertNotIn('edge_groups', policy)
        legacy = {**policy, 'edge_groups': {'test_to_test': []}}
        self.assertNotIn('edge_groups', impact.write_baseline(current, legacy))

    def test_preexisting_cycle_missing_from_ratchet_is_not_a_pr_regression(self):
        policy = impact.baseline(graph())
        current = graph({'//tests:small': ['//pkg:a.py'], '//tests:other': ['//pkg:b.py'],
                         '//cycle:a': ['//cycle:b'], '//cycle:b': ['//cycle:a']})
        self.assertEqual(impact.check(current, current, set(), policy)['errors'], [])

    def test_conftest_reclassification_does_not_make_an_existing_edge_new(self):
        deps = {'//tests:small': ['//pkg:a.py'], '//tests:other': ['//pkg:b.py'],
                '//tests:conftest': ['//pkg:lib'], '//pkg:lib': []}
        base = graph(deps)
        head = graph({**deps, '//pkg:lib': ['//pkg:a.py']})
        result = impact.check(base, head, set(), impact.baseline(base))
        self.assertEqual(result['errors'], [])

    def test_deleting_an_existing_forbidden_edge_needs_no_baseline_update(self):
        head = graph()
        base = graph({'//tests:small': ['//pkg:a.py'], '//tests:other': ['//pkg:b.py'],
                      '//tests:conftest.py': ['//pkg:a.py']})
        policy = {**impact.baseline(base), 'edge_groups': base['edge_groups']}
        self.assertEqual(impact.check(base, head, set(), policy)['errors'], [])

    def test_metrics_include_unused_sources_and_loc_weighting(self):
        current = graph(sources=('//pkg:a.py', '//pkg:b.py', '//pkg:unused.py'),
                        loc={'//pkg:a.py': 8, '//pkg:b.py': 1, '//pkg:unused.py': 1})
        measured = current
        self.assertAlmostEqual(measured['u_pct'], 100 / 3)
        self.assertAlmostEqual(measured['loc_u_pct'], 45)
        self.assertEqual([measured[key] for key in ('reach_median', 'reach_p90', 'reach_max')], [1, 1, 1])
        self.assertEqual(measured['scc_max'], 1)

    def test_closure_handles_diamonds_and_cycles_without_double_counting(self):
        current = graph({'//tests:small': ['//pkg:lib'], '//tests:other': ['//pkg:b.py'],
                         '//pkg:lib': ['//pkg:a.py', '//pkg:hub'],
                         '//pkg:hub': ['//pkg:lib', '//pkg:a.py']})
        self.assertEqual(set(current['reach']['//tests:small']), {'//pkg:a.py'})
        self.assertEqual(current['scc_members'], ['//pkg:hub', '//pkg:lib'])

    def test_new_broad_test_fails_and_names_a_hub_and_added_edge(self):
        base = graph()
        head = graph({'//tests:small': ['//pkg:a.py'], '//tests:other': ['//pkg:b.py'],
                      '//tests:new': ['//pkg:hub'], '//pkg:hub': ['//pkg:a.py', '//pkg:b.py']},
                     tests=('//tests:small', '//tests:other', '//tests:new'))
        result = impact.check(base, head, {'//tests:new'}, impact.baseline(base))
        self.assertTrue(any('reach 2 exceeds budget 1' in error for error in result['errors']))
        self.assertIn('//tests:new', result['explain_targets'])
        self.assertIn(['//tests:new', '//pkg:hub'], result['added_edges'])
        self.assertIn('new tests 1', result['summary'])

    def test_changed_test_can_keep_its_own_base_reach_above_global_budget(self):
        current = graph({'//tests:small': ['//pkg:a.py', '//pkg:b.py'],
                         '//tests:other': ['//pkg:b.py']})
        policy = impact.baseline(current)
        policy['reach_budget'] = 1
        self.assertFalse(impact.check(current, current, set(), policy)['errors'])
        self.assertFalse(impact.check(current, current, {'//tests:small'}, policy)['errors'])

    def test_new_test_can_inherit_its_package_above_the_global_budget(self):
        deps = {'//hub:old': ['//pkg:a.py', '//pkg:b.py'], '//small:test': ['//pkg:a.py']}
        base = graph(deps, tests=('//hub:old', '//small:test'))
        head = graph({**deps, '//hub:new': deps['//hub:old']},
                     tests=('//hub:old', '//hub:new', '//small:test'))
        policy = impact.baseline(base)
        policy['reach_budget'] = 1
        self.assertFalse(impact.check(base, head, {'//hub:new'}, policy)['errors'])

    def test_new_hub_sources_may_raise_u_without_structural_regression(self):
        deps = {'//hub:test': ['//hub:lib'], '//hub:lib': ['//pkg:a.py'],
                '//other:test': ['//hub:lib'], '//small:test': ['//pkg:b.py']}
        base = graph(deps, tests=('//hub:test', '//other:test', '//small:test'))
        sources = ('//pkg:a.py', '//pkg:b.py', '//pkg:new1.py', '//pkg:new2.py', '//pkg:new3.py')
        head = graph({**deps, '//hub:lib': [source for source in sources if source != '//pkg:b.py']},
                     tests=('//hub:test', '//other:test', '//small:test'), sources=sources)
        self.assertGreater(head['u_pct'], base['u_pct'])
        self.assertFalse(impact.check(base, head, set(), impact.baseline(base))['errors'])

    def test_new_test_to_test_and_conftest_edges_fail(self):
        base = graph()
        for source, target in [('//tests:small', '//tests:other'),
                               ('//tests:conftest.py', '//pkg:a.py')]:
            head = graph({'//tests:small': ['//pkg:a.py'], '//tests:other': ['//pkg:b.py'],
                          source: [target]})
            result = impact.check(base, head, set(), impact.baseline(base))
            self.assertTrue(any(('new test_to_test edge' in error or 'new conftest_source edge' in error) for error in result['errors']), result)

    def test_conftest_rule_dependency_is_an_import_even_when_source_is_a_leaf(self):
        base = graph({'//tests:small': ['//pkg:a.py'], '//tests:other': ['//pkg:b.py'],
                      '//tests:conftest': ['//tests:conftest.py'], '//pkg:lib': ['//pkg:a.py']})
        deps = {}
        for source, target in base['edges']:
            deps.setdefault(source, []).append(target)
        deps['//tests:conftest'].append('//pkg:lib')
        head = graph(deps)
        result = impact.check(base, head, set(), impact.baseline(base))
        self.assertTrue(any(('new test_to_test edge' in error or 'new conftest_source edge' in error) for error in result['errors']), result)

    def test_packaged_conftest_data_does_not_turn_a_test_into_a_conftest_import(self):
        deps = {'//tests:small': ['//pkg:a.py', '//tests:conftest.py'],
                '//tests:other': ['//pkg:b.py']}
        base = graph(deps)
        head = graph({**deps, '//tests:small': deps['//tests:small'] + ['//pkg:b.py']})
        result = impact.check(base, head, set(), impact.baseline(base))
        self.assertFalse(any(('new test_to_test edge' in error or 'new conftest_source edge' in error) for error in result['errors']), result)

    def test_existing_source_expansion_reports_added_edge_and_fanout_growth(self):
        base = graph()
        head = graph({'//tests:small': ['//pkg:a.py', '//pkg:b.py'],
                      '//tests:other': ['//pkg:b.py']})
        result = impact.check(base, head, set(), impact.baseline(base))
        self.assertTrue(any('existing source fanout grew' in error for error in result['errors']))
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
        diagnostic = '\n'.join(result['errors'])
        for member in ('//a:x', '//a:y', '//b:x', '//b:y'):
            self.assertIn(member, diagnostic)
        self.assertIn('//a:x -> //b:x', diagnostic)
        grown = graph({**deps, '//a:y': ['//a:z'], '//a:z': ['//a:x']})
        result = impact.check(base, grown, set(), impact.baseline(base))
        self.assertTrue(any('SCC grew or merged' in error for error in result['errors']))
        for harmless in ({**deps, '//a:y': []},
                         {**deps, '//a:x': ['//a:y', '//a:z'], '//a:z': []}):
            with self.subTest(deps=harmless):
                result = impact.check(base, graph(harmless), set(), impact.baseline(base))
                self.assertFalse(result['errors'])

    def test_u_gains_are_informational_and_baseline_keeps_the_minimum(self):
        base = graph()
        head = graph({'//tests:small': [], '//tests:other': ['//pkg:b.py']})
        policy = impact.baseline(base)
        self.assertFalse(impact.check(base, head, set(), policy)['errors'])
        improved = impact.write_baseline(head, policy)
        self.assertEqual(improved['figures']['u_pct'], 25)
        self.assertEqual(impact.write_baseline(base, improved)['figures']['u_pct'], 25)
        self.assertFalse(impact.check(base, head, set(), improved)['errors'])

    def test_baseline_writer_still_refuses_a_new_cycle(self):
        base = graph()
        head = graph({'//tests:small': ['//pkg:a.py'], '//tests:other': ['//pkg:b.py'],
                      '//cycle:a': ['//cycle:b'], '//cycle:b': ['//cycle:a']})
        with self.assertRaisesRegex(ValueError, 'refuses losses'):
            impact.write_baseline(head, impact.baseline(base))

    def new_crate(self):
        test = '//crates/controller:test_controller'
        old = '//crates/controller:src/lib.rs'
        new = '//crates/contracts:src/lib.rs'
        base = graph({test: [old]}, tests=(test,), sources=(old,))
        head = graph({test: [old, '//crates/contracts:lib'],
                      '//crates/contracts:lib': [new]}, tests=(test,), sources=(old, new))
        return base, head, test

    def test_newly_reached_crate_without_exception_fails(self):
        base, head, test = self.new_crate()
        result = impact.check(base, head, {test}, impact.baseline(base), {}, {})
        self.assertTrue(any('reach' in error for error in result['errors']))
        self.assertIn(['//crates/contracts:lib', '//crates/contracts:src/lib.rs'], result['added_edges'])

    def test_reasoned_addition_excuses_reach_and_is_printed(self):
        base, head, test = self.new_crate()
        entries = {f'reach:{test}': 'Extract the contracts crate for #730.'}
        for changed in (set(), {test}):
            with self.subTest(changed=changed):
                result = impact.check(base, head, changed, impact.baseline(base), entries, {})
                self.assertEqual(result['errors'], [])
                for key, reason in entries.items():
                    self.assertIn(key, result['summary'])
                    self.assertIn(reason, result['summary'])

    def test_allowlist_requires_issue_reasons_and_keeps_existing_reasons(self):
        base = graph()
        policy = impact.baseline(base)
        key = 'reach://tests:small'
        for reason in ('', '  ', None, 'integration', '#0', 'word#730x'):
            with self.subTest(reason=reason):
                result = impact.check(base, base, set(), policy, {key: reason}, {})
                self.assertTrue(any('reason' in error for error in result['errors']))
        result = impact.check(base, base, set(), policy, {key: 'Changed for #730.'},
                              {key: 'Original for #730.'})
        self.assertTrue(any('reason' in error for error in result['errors']))
        self.assertEqual(impact.check(base, base, set(), policy, {}, {key: 'Original for #730.'})['errors'], [])

    def test_move_into_new_crate_nets_against_origin(self):
        test = '//crates/controller:test_controller'
        old, new = '//crates/controller:src/contracts.rs', '//crates/contracts:src/lib.rs'
        base = graph({test: ['//crates/controller:lib'], '//crates/controller:lib': [old]},
                     tests=(test,), sources=(old,))
        head = graph({test: ['//crates/contracts:lib'], '//crates/contracts:lib': [new]},
                     tests=(test,), sources=(new,))
        base['source_identities'], head['source_identities'] = {old: 'same-bytes'}, {new: 'same-bytes'}
        result = impact.check(base, head, {test}, impact.baseline(base), {}, {})
        self.assertEqual(result['errors'], [])
        self.assertEqual(result.get('source_moves'), {new: old})
        self.assertIn(['//crates/contracts:lib', new], result['edge_diff']['added']['rule_to_source'])
        self.assertEqual(result['head']['reach'][test], [new])

    def test_moving_a_file_does_not_hide_fanout_growth(self):
        old, new, other = '//old:a.py', '//new:a.py', '//other:b.py'
        base = graph({'//tests:small': [old], '//tests:other': [other]}, sources=(old, other))
        head = graph({'//tests:small': [new], '//tests:other': [new]}, sources=(new, other))
        base['source_identities'], head['source_identities'] = {old: 'same'}, {new: 'same'}
        result = impact.check(base, head, set(), impact.baseline(base))
        self.assertTrue(any('existing source fanout grew' in error for error in result['errors']))

    def test_move_does_not_grant_inheritance_to_destination_package_additions(self):
        test = '//tests:small'
        old, moved, added = '//old:a.py', '//new:a.py', '//new:b.py'
        base = graph({test: [old]}, tests=(test,), sources=(old,))
        head = graph({test: [moved, added]}, tests=(test,), sources=(moved, added))
        base['source_identities'] = {old: 'a-bytes'}
        head['source_identities'] = {moved: 'a-bytes', added: 'b-bytes'}
        for changed in (set(), {test}):
            with self.subTest(changed=changed):
                result = impact.check(base, head, changed, impact.baseline(base))
                self.assertTrue(any('reach' in error and added in error for error in result['errors']),
                                result['errors'])
                self.assertEqual(result['source_moves'], {moved: old})

    def test_move_keeps_original_package_inheritance(self):
        test = '//tests:small'
        old, moved, added = '//old:a.py', '//new:a.py', '//old:b.py'
        base = graph({test: [old]}, tests=(test,), sources=(old,))
        head = graph({test: [moved, added]}, tests=(test,), sources=(moved, added))
        base['source_identities'] = {old: 'a-bytes'}
        head['source_identities'] = {moved: 'a-bytes', added: 'b-bytes'}
        result = impact.check(base, head, {test}, impact.baseline(base))
        self.assertEqual(result['errors'], [])
        self.assertEqual(result['source_moves'], {moved: old})

    def test_surviving_and_unused_duplicates_make_identity_ambiguous(self):
        test = '//tests:small'
        old, kept, new = '//old:a.py', '//keep:empty.py', '//new:a.py'
        for kept_base, kept_head, used in (
                ('empty-bytes', 'empty-bytes', True),
                ('empty-bytes', 'empty-bytes', False),
                ('empty-bytes', 'other-bytes', False),
                ('other-bytes', 'empty-bytes', False)):
            with self.subTest(kept_base=kept_base, kept_head=kept_head, used=used):
                base = graph({test: [old, kept] if used else [old]},
                             tests=(test,), sources=(old, kept))
                head = graph({test: [new, kept] if used else [new]},
                             tests=(test,), sources=(new, kept))
                base['source_identities'] = {old: 'empty-bytes', kept: kept_base}
                head['source_identities'] = {new: 'empty-bytes', kept: kept_head}
                result = impact.check(base, head, set(), impact.baseline(base))
                self.assertTrue(any('reach' in error for error in result['errors']), result['errors'])
                self.assertEqual(result['source_moves'], {})

    def test_multiple_added_duplicates_are_not_moves_even_when_unused(self):
        test = '//tests:small'
        old, new, duplicate = '//old:a.py', '//new:a.py', '//new:duplicate.py'
        base = graph({test: [old]}, tests=(test,), sources=(old,))
        base['source_identities'] = {old: 'same-bytes'}
        for used in (False, True):
            with self.subTest(used=used):
                head = graph({test: [new, duplicate] if used else [new]},
                             tests=(test,), sources=(new, duplicate))
                head['source_identities'] = {new: 'same-bytes', duplicate: 'same-bytes'}
                result = impact.check(base, head, set(), impact.baseline(base))
                self.assertTrue(any('reach' in error for error in result['errors']), result['errors'])
                self.assertEqual(result['source_moves'], {})

    def test_multiple_simultaneous_moves_credit_only_the_matched_sources(self):
        test = '//tests:small'
        old_a, old_b = '//old:a.py', '//old:b.py'
        new_a, new_b = '//new:a.py', '//other:b.py'
        base = graph({test: [old_a, old_b]}, tests=(test,), sources=(old_a, old_b))
        head = graph({test: [new_a, new_b]}, tests=(test,), sources=(new_a, new_b))
        base['source_identities'] = {old_a: 'a-bytes', old_b: 'b-bytes'}
        head['source_identities'] = {new_a: 'a-bytes', new_b: 'b-bytes'}
        result = impact.check(base, head, {test}, impact.baseline(base))
        self.assertEqual(result['errors'], [])
        self.assertEqual(result['source_moves'], {new_a: old_a, new_b: old_b})
        self.assertEqual(result['fanout_growth'], [])
        self.assertEqual(result['base']['reach'][test], sorted([old_a, old_b]))
        self.assertEqual(result['head']['reach'][test], sorted([new_a, new_b]))
        self.assertEqual(result['edge_diff']['added']['rule_to_source'],
                         [[test, new_a], [test, new_b]])

    def test_copies_changed_bytes_and_ambiguous_identity_are_not_moves(self):
        old, new = '//old:a.py', '//new:a.py'
        for keep_old, old_hash, new_hash, extra in (
                (True, 'same', 'same', {}),
                (False, 'before', 'after', {}),
                (False, 'same', 'same', {'//old:duplicate.py': 'same'}),
                (False, None, None, {})):
            with self.subTest(keep_old=keep_old, old_hash=old_hash, extra=extra):
                base = graph({'//tests:small': [old, *extra]}, tests=('//tests:small',),
                             sources=(old, *extra))
                head = graph({'//tests:small': [new]}, tests=('//tests:small',),
                             sources=(new, old) if keep_old else (new,))
                base['source_identities'] = {old: old_hash, **extra}
                head['source_identities'] = {new: new_hash}
                result = impact.check(base, head, set(), impact.baseline(base))
                self.assertEqual(result.get('source_moves'), {})

    def test_reach_exception_keeps_fanout_cycle_and_edge_checks(self):
        base = graph()
        head = graph({'//tests:small': ['//pkg:a.py', '//tests:other'],
                      '//tests:other': ['//pkg:b.py'],
                      '//cycle:a': ['//cycle:b'], '//cycle:b': ['//cycle:a']})
        result = impact.check(base, head, {'//tests:small'}, impact.baseline(base),
                              {'reach://tests:small': 'Contract extraction for #730.'}, {})
        for message in ('existing source fanout grew', 'SCC grew or merged', 'new test_to_test edge'):
            self.assertTrue(any(message in error for error in result['errors']), result['errors'])

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
