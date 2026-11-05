"""Remote-only A4 qualification against real enqueue source mutations.

Python only seeds source faults and launches the required Bazel proof targets.
Run on a disposable BuildBuddy runner, never in a development checkout.
"""
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'kernel/tools'))
import safety_qualification  # noqa: E402

DECIDE = 'crates/mergify/src/decide/enqueue.rs'
safety_qualification.CASES = {
    'ack_independent_sample': (DECIDE,
                               'let Some(recovery) = issue.requeue_recovery else',
                               'let Some(recovery) = issue.recovery else',
                               '//crates/mergify:verus_enqueue_ack_sound'),
    'ack_requeue': (DECIDE,
                    'before.requeue.is_none() && read.check_changed',
                    'read.check_changed',
                    '//crates/mergify:verus_enqueue_ack_sound'),
    'ack_state': (DECIDE,
                  'state.confirmed = true;',
                  '// mutant: omit acknowledgement state',
                  '//crates/mergify:verus_enqueue_ack_sound'),
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
    safety_qualification.main()
