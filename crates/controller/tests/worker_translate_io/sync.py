"""Refresh current worker-translation copies without changing frozen references."""
from pathlib import Path

HERE = Path(__file__).resolve().parent
SOURCE = HERE.parent.parent / 'src'


def main():
    source = (SOURCE / 'lib.rs').read_text()
    application = source.index('    fn apply(')
    application_end = source.index('    fn record_kernel_result(', application) - 1
    (HERE.parent / 'delivery_machine/planner-apply.rs.txt').write_text(source[application:application_end])
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
    current = (HERE / 'rp-current.rs').read_text()
    first = current.index('            Observation::WorkerFailed {')
    last = current.index('        };\n        *detail', first)
    (HERE / 'rp-current.rs').write_text(current[:first] + arm + current[last:])

    production = (SOURCE / 'adapters/workers/protocol.rs').read_text()
    frozen = (HERE / 'protocol.rs').read_text()
    first = frozen.index('pub(crate) fn map_reply(')
    last = frozen.index('pub(crate) fn relative_file(', first)
    start = production.index('pub(crate) fn map_reply(')
    end = production.index('pub(crate) fn relative_file(', start)
    (HERE / 'protocol-rp.rs').write_text(frozen[:first].replace('Kernel(Reply), Made(String)', 'Kernel(Reply), Planner(PlannerResult), Made(String)') + production[start:end] + frozen[last:])


if __name__ == '__main__':
    main()
