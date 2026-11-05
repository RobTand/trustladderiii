"""Seed one production-body mutant; the matching Verus group must reject it."""
import argparse
from pathlib import Path

MUTANTS = {
    'quota_model': ('ModelField::GradeModel if postmortem => QuotaModel::Grade',
                    'ModelField::GradeModel if postmortem => QuotaModel::Implicit'),
    'reply_step': ('if line_end != output_end', 'if line_end == output_end'),
    'progress_due': ('offset > recorded', 'offset >= recorded'),
    'progress_signal': ('if changed', 'if !changed'),
    'claim_decision': ('Claim::Cancelled => ClaimDecision::Retire',
                       'Claim::Cancelled => ClaimDecision::AwaitClaim'),
    'refresh_gate': ('if reconcilable', 'if !reconcilable'),
    'end_decision': ('Ending::Pending => EndDecision::KeepActive',
                     'Ending::Pending => EndDecision::Retire'),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('property', choices=[*MUTANTS, 'quota_position_equality', 'all'])
    args = parser.parse_args()
    crate = Path(__file__).resolve().parent.parent
    source = crate / 'src/worker_events.rs'
    if args.property == 'quota_position_equality':
        # Restore the real shell byte-equality gate. The extra scratch-only
        # input transports its result; the positional contract stays unchanged.
        text = source.read_text()
        start = text.index('pub fn reply_step(')
        end = text.index('\n}\n', start) + 3
        body = text[start:end]
        body = body.replace('    output_end: usize,',
                            '    output_end: usize,\n    last_line_equal: bool,')
        body = body.replace('if line_end != output_end', 'if !last_line_equal')
        source.write_text(text[:start] + body + text[end:])
        root = crate.parent.parent
        for file in ['crates/controller/src/adapters/workers/source.rs',
                     'crates/controller/tests/worker_events_io/current.rs']:
            path = root / file
            text = path.read_text()
            old = 'reply_step(parsed.event, reply.is_some(), line_end, bytes.len())'
            new = ("reply_step(parsed.event, reply.is_some(), line_end, bytes.len(), "
                   "bytes.split_inclusive(|b| *b == b'\\n').next_back() == Some(line))")
            if text.count(old) != 1:
                raise SystemExit(f'{file}: expected one shell gate')
            path.write_text(text.replace(old, new))
        path = crate / 'verus/worker_events_witnesses.rs'
        lines = path.read_text().splitlines(keepends=True)
        for index, line in enumerate(lines):
            if ' = reply_step(' in line:
                end = line.rindex(');')
                lines[index] = line[:end] + ', true' + line[end:]
        path.write_text(''.join(lines))
        return
    text = source.read_text()
    for name in MUTANTS if args.property == 'all' else [args.property]:
        start = text.index('pub fn ' + name + '(')
        end = text.index('\n}\n', start) + 3
        old, new = MUTANTS[name]
        body = text[start:end]
        if body.count(old) != 1:
            raise SystemExit(f'{name}: expected one production mutation site')
        text = text[:start] + body.replace(old, new) + text[end:]
    source.write_text(text)


if __name__ == '__main__':
    main()
