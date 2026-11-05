"""#743 M1b: qualify production mutants using the remote kernel adapter."""
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'kernel/tools'))
import safety_qualification  # noqa: E402

safety_qualification.CASES = {'q52': ('crates/mergify/src/decide/translate.rs',
         'Kind::CheckMissing if node.enqueuing() && approved_head',
         'Kind::CheckMissing if approved_head',
         '//crates/mergify:verus_translate'),
 'translate_closed': ('crates/mergify/src/decide/translate.rs',
                      '(!matches!(kind, Kind::Merged { .. }) || node.merged)',
                      '!matches!(kind, Kind::Merged { .. })',
                      '//crates/mergify:verus_translate'),
 'translate_head_ci': ('crates/mergify/src/decide/translate.rs',
                       'return node.phase == Phase::Queued || node.enqueuing();',
                       'return node.phase != Phase::Closed;',
                       '//crates/mergify:verus_translate'),
 'translate_route': ('crates/mergify/src/decide/translate.rs',
                     'if kind.names_attempt_head() && !head_current {',
                     'if kind.names_attempt_head() && !head_current && approved_head {',
                     '//crates/mergify:verus_translate'),
 'translate_route_2': ('crates/mergify/src/decide/translate.rs',
                       'Self::CiFailed { .. } | Self::Conflict | Self::DequeuedByCommand',
                       'Self::CiFailed { .. } | Self::DequeuedByCommand',
                       '//crates/mergify:verus_translate'),
 'translate_route_3': ('crates/mergify/src/decide/translate.rs',
                       'Self::CiFailed { .. } | Self::Conflict | Self::DequeuedByCommand',
                       'Self::CiFailed { .. } | Self::Conflict',
                       '//crates/mergify:verus_translate'),
 'ci_classify': ('crates/mergify/src/decide/translate.rs',
                 'outside && !repeated',
                 'outside',
                 '//crates/mergify:verus_translate'),
 'person_facts_total': ('crates/mergify/src/decide/facts.rs',
                        'if event.actor != Actor::Person {',
                        'if event.actor == Actor::Missing {',
                        '//crates/mergify:verus_check_facts'),
 'facts_outcome': ('crates/mergify/src/decide/facts.rs',
                   'head_infra.unwrap_or(true)',
                   'head_infra.unwrap_or(false)',
                   '//crates/mergify:verus_check_facts'),
 'suites_scan': ('crates/mergify/src/decide/facts.rs',
                 'decide_while! { (i < suites.len()) [',
                 'decide_while! { (false) [',
                 '//crates/mergify:verus_check_facts'),
 'merge_wait': ('crates/mergify/src/decide/facts.rs',
                'if expected_head\n        && match check',
                'if false\n        && match check',
                '//crates/mergify:verus_check_facts'),
 'person_merge': ('crates/mergify/src/decide/facts.rs',
                  'let mut merge_seen = merged;',
                  'let mut merge_seen = false;',
                  '//crates/mergify:verus_check_facts'),
 'queue_report_eager': ('crates/mergify/src/decide/facts.rs',
                        'Some(infra) => infra,\n                None => ci_infra(check),',
                        'Some(infra) => { let _ = ci_infra(check); infra },\n'
                        '                None => ci_infra(check),',
                        '//crates/mergify:verus_check_facts')}

if __name__ == '__main__':
    safety_qualification.main()
