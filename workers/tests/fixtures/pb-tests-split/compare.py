"""Compare complete Bazel pytest test.log listings before and after a split.

Usage: python compare.py BEFORE_LOG AFTER_LOG [AFTER_LOG ...]
Fetch each log with /home/rob/tmp/tl3-build/bb_logs.py and a line limit large
enough to include every PASSED line. Supply each target's log exactly once.
"""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re


PASSED = re.compile(r'^tests/test_pb_tests(?:_\w+)?\.py::(.+) PASSED\s+\[\s*\d+%\]$')
DURATION = re.compile(r'^(\d+\.\d+)s (\w+)\s+tests/test_pb_tests(?:_\w+)?\.py::(.+)$')
SUMMARY = re.compile(r'^=+ (\d+) passed in ')


def read(paths):
    nodes = Counter()
    durations = []
    passed = 0
    for path in paths:
        for line in path.read_text().splitlines():
            if match := PASSED.match(line):
                nodes[match[1]] += 1
            if match := DURATION.match(line):
                durations.append((float(match[1]), match[2], match[3]))
            if match := SUMMARY.match(line):
                passed += int(match[1])
    if not nodes or sum(nodes.values()) != passed:
        raise SystemExit(f'incomplete listing: {sum(nodes.values())} node IDs, {passed} passed')
    if duplicates := [node for node, count in nodes.items() if count != 1]:
        raise SystemExit(f'duplicate node IDs: {duplicates}')
    return nodes, sorted(durations, reverse=True)[:10]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('before', type=Path)
    parser.add_argument('after', nargs='+', type=Path)
    args = parser.parse_args()
    before, old_durations = read([args.before])
    after, new_durations = read(args.after)
    if before != after:
        raise SystemExit(json.dumps({'missing': sorted(before - after),
                                     'added': sorted(after - before)}, indent=2))
    digest = hashlib.sha256(('\n'.join(sorted(before)) + '\n').encode()).hexdigest()
    print(json.dumps({'equal': True, 'cases': len(before), 'sha256': digest,
                      'before_top10': old_durations, 'after_top10': new_durations}, indent=2))


if __name__ == '__main__':
    main()
