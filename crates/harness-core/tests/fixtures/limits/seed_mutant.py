"""Seed one #835 production mutant on BuildBuddy; never run proofs on a host."""
import argparse
from pathlib import Path

MUTANTS = {
    'arithmetic': ('measured_bytes(a as u128 + b as u128)', 'measured_bytes(a as u128)'),
    'line': ('collection <= MAX_REPORT_LINE_BYTES && largest_phase <= MAX_REPORT_LINE_BYTES',
             'collection < MAX_REPORT_LINE_BYTES && largest_phase <= MAX_REPORT_LINE_BYTES'),
    'accumulation': ('multiply_cost(if forced { 3 } else { 2 },',
                     'multiply_cost(if forced { 2 } else { 2 },'),
    'delay': ('&& exhausted == (bits == 0x4024000000000000)\n',
              '&& exhausted != (bits == 0x4024000000000000)\n'),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('group', choices=MUTANTS)
    args = parser.parse_args()
    source = Path('crates/harness-core/src/limits.rs')
    text = source.read_text()
    before, after = MUTANTS[args.group]
    # Formatting can split the multiplication call across lines.
    if args.group == 'accumulation':
        before, after = 'if forced { 3 } else { 2 }', 'if forced { 2 } else { 2 }'
    if text.count(before) != 1:
        raise SystemExit(f'{args.group}: expected exactly one production expression')
    source.write_text(text.replace(before, after))
    print(f'{args.group}: {before!r} -> {after!r}')


if __name__ == '__main__':
    main()
