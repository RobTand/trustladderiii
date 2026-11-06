"""Refresh current worker-translation copies without changing frozen references."""
from pathlib import Path

HERE = Path(__file__).resolve().parent
SOURCE = HERE.parent.parent / 'src'


def main():
    source = (SOURCE / 'lib.rs').read_text()
    start = source.index('            Observation::WorkerFailed {')
    end = source.index('            Observation::Plan {', start)
    quota_comment = (
        '                // Q53 / #828: unknown legacy identity/order is not success evidence.\n'
        '                // Only model_success_evidence can clear ordered quota; reset and\n'
        '                // operator clear remain available for pre-upgrade holds.\n'
    )
    arm = source[start:end]
    assert arm.count(quota_comment) == 1
    # Keep #828's separately pinned historical clear in this extraction comparison.
    arm = arm.replace(quota_comment, (HERE / 'legacy-quota-clear.rs.txt').read_text())
    current = (HERE / 'current.rs').read_text()
    helper = source[source.index('    fn footprint('):source.index('    fn operations_comment(')]
    if '    fn footprint(' in current:
        first = current.index('    fn footprint(')
        last = current.index('    fn translate(', first)
        current = current[:first] + helper + current[last:]
    else:
        current = current.replace('impl Shell<\'_> {\n', 'impl Shell<\'_> {\n' + helper, 1)
    first = current.index('            Observation::WorkerFailed {')
    last = current.index('        };\n        *detail', first)
    (HERE / 'current.rs').write_text(current[:first] + arm + current[last:])

    production = (SOURCE / 'adapters/workers/protocol.rs').read_text()
    frozen = (HERE / 'protocol.rs').read_text()
    first = frozen.index('pub(crate) fn map_reply(')
    last = frozen.index('pub(crate) fn relative_file(', first)
    start = production.index('pub(crate) fn map_reply(')
    end = production.index('pub(crate) fn relative_file(', start)
    current = frozen[:first] + production[start:end] + frozen[last:]
    first = current.index('pub(crate) fn relative_file(')
    last = current.index('fn validators()', first)
    start = production.index('pub(crate) fn relative_file(')
    end = production.index('\n#[cfg(test)]', start)
    current = current[:first] + production[start:end] + '\n' + current[last:]
    (HERE / 'protocol-current.rs').write_text(current)


if __name__ == '__main__':
    main()
