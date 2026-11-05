"""#743 review r1: qualify equivalence tests against disposable source mutants."""
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'kernel/tools'))
import safety_qualification  # noqa: E402


safety_qualification.CASES = {
    'differential_timestamp': ('crates/mergify/src/decide/observe.rs',
                               'finished.nanos >= dequeued.nanos',
                               'finished.nanos > dequeued.nanos',
                               '//crates/mergify:test_test_observe_differential'),
    'differential_selection_tie': ('crates/mergify/src/decide/observe.rs',
                                   'check.id >= checks[best].id',
                                   'check.id > checks[best].id',
                                   '//crates/mergify:test_test_observe_differential'),
}

if __name__ == '__main__':
    safety_qualification.main()
