"""Seed one production-body mutant; the matching Verus group must reject it."""
import argparse
from pathlib import Path

MUTANTS = {
    'quota_model': ('ModelField::GradeModel if postmortem => QuotaModel::Grade',
                    'ModelField::GradeModel if postmortem => QuotaModel::Implicit'),
    'reply_step': ('if !last_line_equal', 'if last_line_equal'),
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
    parser.add_argument('property', choices=[*MUTANTS, 'all'])
    args = parser.parse_args()
    source = Path(__file__).resolve().parent.parent / 'src/worker_events.rs'
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
