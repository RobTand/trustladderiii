"""Split a libtest binary across Bazel shards by measured test time (#731).

rust_runner.py lists the binary's tests, keeps this shard's share and runs them
with --exact. The weights come from test logs, never from hand-edited numbers:

    python3 tools/bazel/libtest_shards.py BINARY TEST_LOG... > /tmp/weights.json

then merge the output into tools/bazel/libtest-shards.json. A test missing from
the weights, such as a new one, counts as the median measured test.
"""
import json
from pathlib import Path
import re
import statistics
import sys

sys.path.insert(0, str(Path(__file__).absolute().parents[1] / 'ci'))
import shards  # noqa: E402

WEIGHTS = Path('tools/bazel/libtest-shards.json')


def listed(output):
    """Test names from `--list --format=terse`, which prints `NAME: test`."""
    raise NotImplementedError


def select(names, weights, index, total):
    """This shard's tests, in a partition that covers every name exactly once."""
    raise NotImplementedError


def measured(binary, lines):
    """Seconds per passing test from rust_runner's `[BINARY] {libtest json}` lines.

    A test seen in several logs keeps its largest time, so one quiet run cannot
    hide a slow test from the partition.
    """
    raise NotImplementedError


def main():
    binary, *logs = sys.argv[1:]
    if not logs:
        raise SystemExit('usage: libtest_shards.py BINARY TEST_LOG...')
    lines = [line for log in logs for line in Path(log).read_text().splitlines()]
    print(json.dumps({binary: measured(binary, lines)}, indent=1, sort_keys=True))


if __name__ == '__main__':
    main()
