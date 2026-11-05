"""Project real Rust type declarations into the standalone recovery proof root.

This is proof-build glue. The executable functions are included from production;
no protocol behavior or handwritten type mirror lives here.
"""
import re
import sys
from pathlib import Path


def declaration(source, name):
    match = re.search(r'pub (?:enum|struct) ' + re.escape(name) + r'\s*\{', source)
    if match is None:
        raise RuntimeError(f'missing production type {name}')
    start = match.start()
    end = match.end()
    depth = 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return '#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n' + source[start:end] + '\n'


def main():
    enqueue, check, output = (Path(arg) for arg in sys.argv[1:])
    source = enqueue.read_text()
    constants = re.findall(r'^pub const GRACE_MS: u64 = [^;]+;', source, re.MULTILINE)
    if len(constants) != 1:
        raise RuntimeError('missing production grace constant')
    types = ['Command', 'Recovery', 'CommandRead', 'CommandResult', 'Complete',
             'Failure', 'Effect', 'Need', 'Write']
    root = (declaration(check.read_text(), 'Head') + '\npub mod enqueue {\n'
            'use super::Head;\n' + constants[0] + '\n'
            + '\n'.join(declaration(source, name) for name in types) + '\n}\n'
            '#[path = "../mergify/src/decide/recovery.rs"]\nmod recovery;\nfn main() {}\n')
    output.write_text(root)


if __name__ == '__main__':
    main()
