"""Summarize downloaded rust_runner inventory receipts for Mergify sharding.

Run with one or more inventory.json files from a single BuildBuddy invocation.
"""
import argparse
import json
import math
import re
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('receipts', nargs='+', type=Path)
    parser.add_argument('--logs', nargs='*', type=Path, default=[])
    parser.add_argument('--extra-receipts', nargs='*', type=Path, default=[],
                        help='Additional runs used only to take conservative maximum weights')
    parser.add_argument('--write', action='store_true', help='Update the Rust group weights from a pre-fix run')
    args = parser.parse_args()
    tests = {}
    shards = {}
    for path in args.receipts:
        receipt = json.loads(path.read_text())
        shard = receipt['shard']
        index = shard['index']
        if index in shards:
            raise SystemExit(f'duplicate shard {index}')
        seconds = receipt['seconds']
        overlap = tests.keys() & (seconds.keys() - {'tl3-controller/mergify::__tail__'})
        if overlap:
            raise SystemExit(f'duplicate scenarios: {sorted(overlap)}')
        tests.update({name: value for name, value in seconds.items() if not name.endswith('::__tail__')})
        shards[index] = sum(seconds.values())
    if sorted(shards) != list(range(shard['total'])):
        raise SystemExit('missing shard receipts')
    for path in args.extra_receipts:
        receipt = json.loads(path.read_text())
        for name, seconds in receipt['seconds'].items():
            if not name.endswith('::__tail__'):
                tests[name] = max(tests.get(name, 0), seconds)
    groups = {}
    for path in args.logs:
        for group, seconds in re.findall(r'mergify group (\d+): ([0-9.]+)s', path.read_text()):
            index = int(group)
            groups[index] = max(groups.get(index, 0), math.ceil(float(seconds)))
    if args.write:
        source = Path(__file__).parent.parent / 'mergify.rs'
        cases = Path(__file__).with_name('waiting_requeue.rs').read_text()
        names = re.findall(r'"(\w+)",\s*\1(?: as fn\(\))?,', cases)
        if sorted(groups) != list(range(174)) or len(names) != 9:
            raise SystemExit('expected all 174 ordinary groups and nine waiting-requeue cases')
        weights = [max(1, groups[index]) for index in range(174)]
        weights.extend(max(1, math.ceil(tests[f'tl3-controller/mergify::{name}'])) for name in names)
        rendered = 'let seconds = [\n        ' + ', '.join(map(str, weights)) + ',\n    ];'
        source.write_text(re.sub(r'let seconds = \[[\s\S]*?\];', lambda _: rendered, source.read_text(), count=1))
    print(json.dumps({'group_weights': groups, 'shards': shards, 'slowest': sorted(tests.items(), key=lambda item: -item[1])[:15],
                      'weights': {name: max(1, math.ceil(value)) for name, value in sorted(tests.items())}}, indent=2))


if __name__ == '__main__':
    main()
