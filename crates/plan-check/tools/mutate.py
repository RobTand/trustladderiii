"""Seed production mutations for #820's Verus red evidence. Run on a scratch commit."""
import argparse
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mutant', choices=['star-crosses-slash', 'subject-boundary'])
    args = parser.parse_args()
    if args.mutant == 'star-crosses-slash':
        path = ROOT / 'src/paths/decision.rs'
        before = 'Step::Star => same || left && !separator,'
        after = 'Step::Star => same || left,'
    else:
        path = ROOT / 'src/subjects/decision.rs'
        before = "subject[chosen.len() + 1] == b':'"
        after = "subject[chosen.len() + 1] != b':'"
    content = path.read_text()
    prefix, found, suffix = content.rpartition(before)
    if not found:
        raise SystemExit(f'mutation anchor missing: {path}')
    path.write_text(prefix + after + suffix)
    print(f'{args.mutant}: {path.relative_to(ROOT)}')


if __name__ == '__main__':
    main()
