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
    groups = {}
    for path in args.logs:
        for group, seconds in re.findall(r'mergify group (\d+): ([0-9.]+)s', path.read_text()):
            index = int(group)
            groups[index] = max(groups.get(index, 0), math.ceil(float(seconds)))
    print(json.dumps({'group_weights': groups, 'shards': shards, 'slowest': sorted(tests.items(), key=lambda item: -item[1])[:15],
                      'weights': {name: max(1, math.ceil(value)) for name, value in sorted(tests.items())}}, indent=2))


if __name__ == '__main__':
    main()
