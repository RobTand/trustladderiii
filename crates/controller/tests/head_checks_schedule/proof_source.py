"""Build a Verus root from production definitions and production function bodies.

The executable required.rs module is copied whole. External domain types are
extracted by name from their authoritative modules, rather than mirrored here.
The Q52 fact-kind enum is read from its authoritative translation module.
"""
import pathlib
import sys


def item(source, declaration):
    start = source.index(declaration)
    opening = source.index('{', start)
    depth = 1
    end = opening + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


def domain(source, names):
    return '\n'.join(
        '#[vstd::prelude::verus_verify]\n#[derive(vstd::prelude::Structural)]\n#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n'
        + item(source, 'pub ' + kind + ' ' + name)
        for kind, name in names
    )


def main():
    check, enqueue, required, translate, output = map(pathlib.Path, sys.argv[1:])
    check_source = check.read_text()
    enqueue_source = enqueue.read_text()
    route_source = translate.read_text()
    check_types = domain(check_source, [
        ('enum', 'Status'), ('enum', 'Conclusion'), ('enum', 'Head'),
        ('enum', 'App'), ('enum', 'Name'), ('struct', 'Time'), ('struct', 'CheckView'),
    ])
    enqueue_types = domain(enqueue_source, [
        ('struct', 'Rerun'), ('enum', 'RerunResult'), ('enum', 'RequiredState'),
    ])
    fact_types = domain(route_source, [('enum', 'Kind')])
    output.write_text(
        'mod check {\n' + check_types + '\n}\n'
        'pub use check::*;\n'
        'mod enqueue {\n' + enqueue_types + '\n}\n'
        'mod translate {\n' + fact_types + '\n}\n'
        'mod required {\n' + required.read_text() + '\n}\n'
        'fn main() {}\n')


if __name__ == '__main__':
    main()
