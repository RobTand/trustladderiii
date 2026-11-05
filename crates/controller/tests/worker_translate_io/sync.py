"""Refresh current worker-translation copies without changing frozen references."""
from pathlib import Path

HERE = Path(__file__).resolve().parent
SOURCE = HERE.parent.parent / 'src'


def main():
    source = (SOURCE / 'lib.rs').read_text()
    start = source.index('            Observation::WorkerFailed {')
    end = source.index('            Observation::Plan {', start)
    current = (HERE / 'current.rs').read_text()
    first = current.index('            Observation::WorkerFailed {')
    last = current.index('        };\n        *detail', first)
    (HERE / 'current.rs').write_text(current[:first] + source[start:end] + current[last:])

    production = (SOURCE / 'adapters/workers/protocol.rs').read_text()
    frozen = (HERE / 'protocol.rs').read_text()
    first = frozen.index('pub(crate) fn map_reply(')
    last = frozen.index('pub(crate) fn relative_file(', first)
    start = production.index('pub(crate) fn map_reply(')
    end = production.index('pub(crate) fn relative_file(', start)
    (HERE / 'protocol-current.rs').write_text(frozen[:first] + production[start:end] + frozen[last:])


if __name__ == '__main__':
    main()
