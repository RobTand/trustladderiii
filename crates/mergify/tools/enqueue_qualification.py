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
}

if __name__ == '__main__':
    reference = Path(__file__).resolve().parents[1] / 'tests/enqueue_reference.rs.txt'
    assert hashlib.sha256(reference.read_bytes()).hexdigest() == '898b7d6fb8873079c3a224fc6b1697fe1fd6359d3b6bd9b9f05d451f238826d2', (
        'the verbatim pre-port reference changed')
    safety_qualification.main()
