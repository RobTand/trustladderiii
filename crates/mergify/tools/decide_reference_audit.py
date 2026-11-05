"""#743 M1b: audit verbatim references and the frozen M1a source, without executing it."""
from pathlib import Path
import re
import subprocess

BASE = 'df6cb5367f39281846d007037e55f4d868aafba3'
FROZEN = '010dd404f09f018c71818cd05c2dd368b60348d9'
ROOT = Path(__file__).resolve().parents[3]
FACTS = 'crates/mergify/src/decide/facts.rs'
REFERENCES = 'crates/mergify/tests/decide_differential/reference.rs'
NAMES = ['suites_completed', 'merged_plan', 'queue_outcome', 'person_fact', 'person_facts']


def source_at(commit, path):
    return subprocess.check_output(['git', 'show', commit + ':' + path], cwd=ROOT, text=True)


def function(source, name, *, annotations=False):
    match = re.search(r'pub fn ' + name + r'[<(]', source)
    assert match is not None, name
    start = match.start()
    end = source.index('{', start) + 1
    depth = 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    if annotations:
        start = source.rfind('\n\n', 0, start) + 2
    return source[start:end]


def main():
    original = source_at(BASE, FACTS)
    reference = (ROOT / REFERENCES).read_text()
    current = (ROOT / FACTS).read_text()
    for name in NAMES:
        assert function(original, name) == function(reference, name), name
        print(name + ': verbatim reference at ' + BASE)
    for name in re.findall(r'pub fn (\w+)[<(]', original):
        if function(original, name) != function(current, name):
            assert name in NAMES, 'rewritten body lacks a verbatim differential: ' + name
            print(name + ': changed body covered by test_test_decide_differential')
    frozen = source_at(FROZEN, FACTS)
    for name in ['checks_settled', 'queue_blocked', 'blocking_failure', 'checks_passed']:
        assert function(frozen, name, annotations=True) == function(current, name, annotations=True), name
    for path in ['crates/mergify/src/decide/check.rs', 'crates/mergify/src/decide/observe.rs']:
        assert source_at(FROZEN, path) == (ROOT / path).read_text(), path
    print('M1a frozen types, bodies and annotations unchanged.')


if __name__ == '__main__':
    main()
