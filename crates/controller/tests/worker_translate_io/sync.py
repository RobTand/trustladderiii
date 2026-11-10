"""Refresh current worker-translation copies without changing frozen references."""
from pathlib import Path

HERE = Path(__file__).resolve().parent
SOURCE = HERE.parent.parent / 'src'


def main():
    source = (SOURCE / 'lib.rs').read_text()
    # R2 deliberately retains historical feedback and quota behavior in this
    # seam. Refresh the C-4 NeedsFiles arm, not unrelated moved feedback code.
    current = (HERE / 'current.rs').read_text()
    marker = '                    adapters::workers::protocol::MappedReply::NeedsFiles { paths, fits } => {'
    terminator = '                };\n                if mapped'
    first = current.index(marker)
    last = current.index(terminator, first)
    start = source.index(marker)
    end = source.index(terminator, start)
    (HERE / 'current.rs').write_text(current[:first] + source[start:end] + current[last:])

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

    review = (SOURCE / 'adapters/workers/review.rs').read_text()
    criteria = review[review.index('pub(crate) fn criteria('):review.index('pub(super) fn checkout(')]
    remember = review[review.index('pub(crate) fn remember_addition_requests('):review.index('pub(super) fn fit_issue(')]
    (HERE / 'review-current.rs').write_text('use crate::*;\nuse crate::io::Store;\n'
        + 'pub const REPORT_RULE: &str = "test-report-integrity";\n'
        + 'pub const SCOPE_RULE: &str = "request-scope";\n'
        + 'pub const ISSUE_RULE: &str = "request-coverage";\n'
        + 'pub(crate) use crate::adapters::workers::review::{validate_reply, CitationKind};\n'
        + criteria + remember)



if __name__ == '__main__':
    main()
