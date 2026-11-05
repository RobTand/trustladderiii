"""Refresh checked production copies after editing the worker-output shell."""
from pathlib import Path

HERE = Path(__file__).resolve().parent
SOURCE = HERE.parent.parent / 'src/adapters/workers/source.rs'


def function(source, name):
    at = source.index('fn ' + name + '(')
    start = source.rfind('\n', 0, at) + 1
    line = source[start:at]
    indent = line[:len(line) - len(line.lstrip())]
    end = source.index('\n' + indent + '}\n', at) + len(indent) + 3
    return source[start:end]


def main():
    source = SOURCE.read_text()
    names = ['parse_event', 'decode_event', 'external_wait', 'parse_reply']
    header = '// Function bodies are checked against their pinned source by the harness.\n'
    header += 'use super::*;\nuse super::io::*;\nuse tl3_controller_core::worker_events::*;\n'
    (HERE / 'current.rs').write_text(
        header + 'impl WorkerOutputSource {\n' + function(source, 'refresh') + '}\n'
        + ''.join(function(source, name) for name in names))


if __name__ == '__main__':
    main()
