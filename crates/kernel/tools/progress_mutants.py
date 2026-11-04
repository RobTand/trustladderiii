"""#771 qualification driver. Run only inside an admitted PrismaBuild action."""

import argparse
import hashlib
from pathlib import Path
import re
import shutil
import subprocess
import tempfile


CASES = [
    ('command_id', 'lib.rs', 'self.last_cmd.checked_add(1)', 'self.last_cmd.checked_add(0)'),
    ('credit_balance', 'lib.rs', 'if self.withheld {\n                0', 'if false {\n                0'),
    ('credit_ledger', 'lib.rs',
     'self.start as i64 + self.earned as i64 + self.added as i64 - self.spent as i64',
     'self.start as i64 + self.earned as i64 - self.spent as i64'),
    ('credit_spend', 'lib.rs', 'self.spent += cost;', 'self.spent += cost + 1;'),
    ('credit_topup', 'lib.rs', 'self.added += short as u32;', 'self.added += 0;'),
    ('agenda', 'lib.rs', 'Phase::Unplanned => &[I::Admit],', 'Phase::Unplanned => &[],'),
    ('phase_rank', 'lib.rs', 'Phase::Unplanned => 28,', 'Phase::Unplanned => 27,'),
    ('distance', 'ranks.rs', 'd += 8;', 'd += 7;'),
    ('pending_rank', 'ranks.rs', 'return 3;', 'return 2;'),
    ('sum_update', 'ranks.rs',
     'node_total(nodes.drop_last()) + distance(nodes.last())',
     'node_total(nodes.drop_last()) + 0'),
]


def verify(binary, sysroot, root):
    return subprocess.run(
        [str(binary), '--num-threads', '1', '--rlimit', '30', '--sysroot', str(sysroot),
         str(root), '--edition', '2024'],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, check=False,
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('verifier', type=Path)
    parser.add_argument('sysroot', type=Path)
    parser.add_argument('--case', choices=[case[0] for case in CASES], action='append')
    args = parser.parse_args()
    kernel = Path(__file__).resolve().parents[1]
    baseline = verify(args.verifier, args.sysroot, kernel / 'verus/progress.rs')
    print(baseline.stdout, flush=True)
    if baseline.returncode != 0 or not re.search(
        r'verification results:: [1-9][0-9]* verified, 0 errors', baseline.stdout
    ):
        raise SystemExit('baseline did not verify; mutant rejection would be ambiguous')
    for name, filename, old, new in CASES:
        if args.case and name not in args.case:
            continue
        with tempfile.TemporaryDirectory(prefix=f't771-{name}-') as scratch:
            copy = Path(scratch) / 'kernel'
            shutil.copytree(kernel, copy)
            source = copy / 'src' / filename
            before = source.read_text()
            if old not in before:
                raise SystemExit(f'{name}: seed no longer matches the executable source')
            mutated = before.replace(old, new, 1)
            source.write_text(mutated)
            digest = hashlib.sha256(mutated.encode()).hexdigest()
            result = verify(args.verifier, args.sysroot, copy / 'verus/progress.rs')
            print(f'CASE {name} source_sha256={digest} exit={result.returncode}', flush=True)
            print(result.stdout, flush=True)
            rejected = result.returncode != 0 and re.search(
                r'verification results:: [0-9]+ verified, [1-9][0-9]* errors', result.stdout
            ) and re.search(
                r'postcondition not satisfied|assertion failed|arithmetic overflow|arithmetic underflow',
                result.stdout,
            )
            if not rejected:
                raise SystemExit(f'{name}: expected a failed proof obligation, not a frontend error')
            print(f'REJECTED {name}', flush=True)


if __name__ == '__main__':
    main()
