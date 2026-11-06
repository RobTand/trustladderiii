"""Remote-only B2 qualification against real enqueue source mutations.

Python selects mutations and launches Bazel; the production decisions and the
old/new differential are Rust. Run only on a disposable BuildBuddy runner.
"""
import hashlib
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'kernel/tools'))
import safety_qualification  # noqa: E402

DECIDE = 'crates/mergify/src/decide/enqueue.rs'
safety_qualification.CASES = {
    'differential_stub': (DECIDE,
                          '    let mut out = Effects::new();',
                          '    if !state.confirmed { return (state, Effects::new()); }\n'
                          '    let mut out = Effects::new();',
                          '//crates/mergify:test_test_enqueue_equivalence'),
    'refused_sticky': (DECIDE, 'if refused_intent {', 'if false && refused_intent {',
                       '//crates/mergify:verus_enqueue_refused_sticky'),
    'write_ahead_prepare': (DECIDE,
                            'out.push(Effect::SaveIntent);\n                out.push(if intent.requeue.is_some()',
                            'out.push(if intent.requeue.is_some()',
                            '//crates/mergify:verus_enqueue_write_ahead_label'),
    'write_ahead_command': (DECIDE,
                            'out.push(Effect::SaveRecovery);\n                out.push(Effect::Write(Write::Command));',
                            'out.push(Effect::Write(Write::Command));\n                out.push(Effect::SaveRecovery);',
                            '//crates/mergify:verus_enqueue_write_ahead_command'),
    'write_ahead_rerun': (DECIDE,
                          'out.push(Effect::SaveRerun);\n                out.push(Effect::Write(Write::Rerun));',
                          'out.push(Effect::Write(Write::Rerun));\n                out.push(Effect::SaveRerun);',
                          '//crates/mergify:verus_enqueue_write_ahead_rerun'),
    'recovery_grace': (DECIDE,
                       'now_ms.saturating_sub(since) >= GRACE_MS',
                       'now_ms.saturating_sub(since) >= GRACE_MS / 2',
                       '//crates/mergify:verus_enqueue_recovery_grace'),
    'recovery_reread': (DECIDE, '|| read.paused\n',
                        '|| read.paused\n                || read.waiting && match recovery.eligible_since_ms { '
                        'None => true, Some(since) => read.now_ms.saturating_sub(since) < GRACE_MS }\n',
                        '//crates/mergify:verus_enqueue_recovery_grace'),
}

if __name__ == '__main__':
    reference = Path(__file__).resolve().parents[1] / 'tests/enqueue_reference.rs.txt'
    assert hashlib.sha256(reference.read_bytes()).hexdigest() == '6ddc0fc592d5e34373f316c4031ddae65e7a49ac67a0f02cfa5eedf67dac1121', (
        'the verbatim pre-port reference changed')
    safety_qualification.main()
