"""#743: remote-only real-source mutant qualification for the M1a decisions.

Reuse the kernel adapter's baseline, direct-child, obligation and required-Rust
checks. Python selects source edits and launches Bazel; Rust decides queue state.
"""
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'kernel/tools'))
import safety_qualification  # noqa: E402


DECIDE = 'crates/mergify/src/decide/'
safety_qualification.CASES = {
    'observe_select': (DECIDE + 'observe.rs',
                       'check.id >= checks[best].id', 'check.id > checks[best].id',
                       '//crates/mergify:verus_observe'),
    'observe_merged': (DECIDE + 'observe.rs',
                       'if reported == QueueState::Merged {\n        QueueState::Unknown',
                       'if reported == QueueState::Waiting {\n        QueueState::Unknown',
                       '//crates/mergify:verus_observe'),
    'observe_conflict': (DECIDE + 'observe.rs',
                         '(pull.mergeable == Some(false) || pull.mergeable_state == Mergeable::Dirty)',
                         'pull.mergeable == Some(false)', '//crates/mergify:verus_observe'),
    'observe_rule_concluded': (DECIDE + 'observe.rs',
                               'finished.nanos >= dequeued.nanos',
                               'finished.nanos > dequeued.nanos', '//crates/mergify:verus_observe'),
    'observe_check_state': (DECIDE + 'observe.rs',
                            '    ) {\n        QueueState::FailedCi',
                            '    ) && conclusion.is_none() {\n        QueueState::FailedCi',
                            '//crates/mergify:verus_observe'),
    'observe_merged_eager': (DECIDE + 'observe.rs',
                             '    if pull.merged {\n        return QueueState::Merged;\n    }\n    let report_value = report();',
                             '    let report_value = report();\n    if pull.merged {\n        return QueueState::Merged;\n    }',
                             '//crates/mergify:verus_observe'),
    'observe_check_state_eager': (DECIDE + 'observe.rs',
                                  '    if matches!(\n        conclusion,\n        Some(Conclusion::TimedOut | Conclusion::StartupFailure)\n    ) {\n        QueueState::FailedCi\n    } else {\n        report()\n    }',
                                  '    let state = report();\n    if matches!(\n        conclusion,\n        Some(Conclusion::TimedOut | Conclusion::StartupFailure)\n    ) {\n        QueueState::FailedCi\n    } else {\n        state\n    }',
                                  '//crates/mergify:verus_observe'),
    'checks_passed_total': (DECIDE + 'facts.rs',
                            '|a, b| a.app_key == b.app_key && a.name_key == b.name_key',
                            '|a, b| a.name_key == b.name_key', '//crates/mergify:verus_check_facts'),
    'queue_blocked_total': (DECIDE + 'facts.rs',
                            '            && !check.required\n', '\n',
                            '//crates/mergify:verus_check_facts'),
}

if __name__ == '__main__':
    safety_qualification.main()
