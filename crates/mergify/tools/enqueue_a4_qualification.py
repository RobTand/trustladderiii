"""Remote-only A4 qualification against real enqueue source mutations.

Python only seeds source faults and launches the required Bazel proof targets.
Run on a disposable BuildBuddy runner, never in a development checkout.
"""
import hashlib
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'kernel/tools'))
import safety_qualification  # noqa: E402

DECIDE = 'crates/mergify/src/decide/enqueue.rs'
safety_qualification.CASES = {
    'ack_independent_sample': (DECIDE,
                               'let Some(recovery) = issue.requeue_recovery else',
                               'let Some(recovery) = issue.recovery else',
                               '//crates/mergify:verus_enqueue_ack_sound_issue'),
    'ack_requeue': (DECIDE,
                    'before.requeue.is_none() && read.check_changed',
                    'read.check_changed',
                    '//crates/mergify:verus_enqueue_ack_sound'),
    'ack_state': (DECIDE,
                  'state.confirmed = true;',
                  '// mutant: omit acknowledgement state',
                  '//crates/mergify:verus_enqueue_ack_sound'),
    'dequeue_differential': (DECIDE,
                              '                    Some(queued) => event.id > queued,',
                              '                    Some(queued) => event.id >= queued,',
                              '//crates/mergify:test_test_enqueue_equivalence'),
    'dequeue_fresh': (DECIDE,
                       '                    Some(queued) => event.id > queued,',
                       '                    Some(queued) => event.id >= queued,',
                       '//crates/mergify:verus_enqueue_dequeue_fresh'),
    'stale_command': (DECIDE,
                      'read.head != Head::Current\n                || !read.open',
                      '!read.open',
                      '//crates/mergify:verus_enqueue_stale_head'),
    'stale_label': (DECIDE,
                    '} else if head != Head::Current {\n                Effect::Complete(Complete::Retryable)',
                    '} else if false && head != Head::Current {\n                Effect::Complete(Complete::Retryable)',
                    '//crates/mergify:verus_enqueue_stale_head'),
}

if __name__ == '__main__':
    reference = Path(__file__).resolve().parents[1] / 'tests/dequeue_reference.rs.txt'
    assert hashlib.sha256(reference.read_bytes()).hexdigest() == '1d52b695dce76bc0790af96bd847a50fd5b7253b5577084889b0b4a9657b7d5e', (
        'the verbatim pre-port dequeue reference changed')
    safety_qualification.main()
