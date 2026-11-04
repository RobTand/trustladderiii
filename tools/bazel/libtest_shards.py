"""Split test binaries across Bazel shards by measured time (#731, #742).

rust_runner.py lists a libtest binary's tests, keeps this shard's share and runs
them with --exact. A custom scenario harness reads its group weights from the
same file and assigns whole groups (scenario_shard.rs). The weights come from
test logs, never from hand-edited numbers:

    python3 tools/bazel/libtest_shards.py BINARY TEST_LOG... > weights.json
    python3 tools/bazel/libtest_shards.py --groups BINARY TEST_LOG... > weights.json

then merge the output into tools/bazel/libtest-shards.json. A test or group
missing from the weights, such as a new one, counts as the median measured one.
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
    return [line.removesuffix(': test') for line in output.splitlines() if line.endswith(': test')]


def select(names, weights, index, total):
    """This shard's tests, in a partition that covers every name exactly once."""
    if not 0 <= index < total:
        raise ValueError('Shard must satisfy 0 <= INDEX < TOTAL')
    default = statistics.median(weights.values()) if weights else 1.0
    filled = {name: weights.get(name, default) for name in names}
    chosen = shards.partition(names, total, filled)[index]
    if not chosen:
        raise RuntimeError(f'Empty libtest shard {index}/{total}')
    return sorted(chosen)


def measured(binary, lines):
    """Seconds per passing test from rust_runner's `[BINARY] {libtest json}` lines.

    A test seen in several logs keeps its largest time, so one quiet run cannot
    hide a slow test from the partition.
    """
    prefix = re.compile(r'^\[' + re.escape(binary) + r'\] (\{.*\})\s*$')
    seconds = {}
    for line in lines:
        match = prefix.match(line)
        if not match:
            continue
        try:
            event = json.loads(match[1])
        except json.JSONDecodeError:
            continue
        if event.get('type') == 'test' and event.get('event') == 'ok' and 'exec_time' in event:
            name = event['name']
            seconds[name] = max(seconds.get(name, 0.0), round(event['exec_time'], 3))
    if not seconds:
        raise RuntimeError(f'No passing {binary} tests in the logs')
    return dict(sorted(seconds.items()))


def measured_groups(binary, lines):
    """Seconds per scenario group from scenario_shard.rs's `scenario group NAME: Xs` lines."""
    raise NotImplementedError('#742: measure scenario groups')


def main():
    arguments = sys.argv[1:]
    groups = arguments[:1] == ['--groups']
    binary, *logs = arguments[groups:]
    if not logs:
        raise SystemExit('usage: libtest_shards.py [--groups] BINARY TEST_LOG...')
    lines = [line for log in logs for line in Path(log).read_text().splitlines()]
    weights = measured_groups(binary, lines) if groups else measured(binary, lines)
    print(json.dumps({binary: weights}, indent=1, sort_keys=True))


if __name__ == '__main__':
    main()
