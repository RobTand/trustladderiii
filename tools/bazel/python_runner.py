"""Run one discovered module and write a parity receipt only after success."""
import json
import os
from pathlib import Path
import sys
import unittest

# Import siblings by path: a crate's own tools/ directory can shadow ours.
sys.path.insert(0, str(Path(__file__).parent))
from runtime import prepare  # noqa: E402


def run_unittest(name):
    suite = unittest.defaultTestLoader.loadTestsFromName(name)
    if not suite.countTestCases():
        # Match pytest's exit 5: a module that runs nothing must not pass parity.
        print(f'{name} defines no tests', file=sys.stderr)
        return 5
    return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1


def shard_items(items, index, total):
    """Bazel shard `index` of `total`: round-robin over the sorted node IDs.

    Each shard is a separate process whose collection order may differ (sets,
    hash-seeded parametrize), so the deal ranks node IDs, not positions. Every
    item lands in exactly one shard, in its collected order. Sorted neighbours
    are usually parametrized cases of one test with similar cost, so dealing
    them out keeps shards even.
    """
    rank = {nodeid: position for position, nodeid in enumerate(sorted(item.nodeid for item in items))}
    return [item for item in items if rank[item.nodeid] % total == index]


class Shard:
    """Bazel's test sharding protocol for pytest; see the Bazel test encyclopedia."""

    def __init__(self, index, total):
        self.index, self.total = index, total

    def pytest_collection_modifyitems(self, config, items):
        kept = shard_items(items, self.index, self.total)
        kept_ids = {id(item) for item in kept}
        config.hook.pytest_deselected(items=[item for item in items if id(item) not in kept_ids])
        items[:] = kept


def bazel_shard():
    """(index, total) when Bazel shards this test, else None."""
    total = int(os.environ.get('TEST_TOTAL_SHARDS', '1'))
    if total < 2:
        return None
    if status := os.environ.get('TEST_SHARD_STATUS_FILE'):
        # Tells Bazel this runner selects its own shard instead of running everything.
        Path(status).touch()
    return int(os.environ['TEST_SHARD_INDEX']), total


def main():
    prepare()
    relative = sys.argv[1]
    shard = bazel_shard()
    module = Path(relative).resolve()
    # The deployment tests drive the helper that Bazel built for this test.
    os.environ['TL3_DEPLOY_HELPER'] = str(Path('crates/controller/example_tl3-deploy').resolve())
    sys.path.insert(0, str(module.parent))
    if relative.startswith(('workers/', 'harness/')):
        # Light checks run these projects' pytest from the project directory.
        import pytest
        plugins = [Shard(*shard)] if shard else []
        args = [str(module), '-v', '--durations=10', *sys.argv[2:]]
        if delay := os.environ.get('TEST_SYNC_DELAY_SECONDS'):
            from sync_contention import SyncContention
            plugins.append(SyncContention(float(delay)))
            args.append('-s')
        os.chdir(relative.split('/')[0])
        status = pytest.main(args, plugins=plugins)
    elif shard:
        raise RuntimeError(f'{relative}: sharding needs a pytest module')
    else:
        status = run_unittest(module.stem)
    if status:
        return int(status)
    # Parity counts modules, so one shard names the module and the others add nothing.
    # A failed shard fails the target, so the module never passes partly run.
    receipt = {'python': [relative] if not shard or shard[0] == 0 else []}
    if shard:
        receipt['shard'] = {'index': shard[0], 'total': shard[1]}
    Path(os.environ['TEST_UNDECLARED_OUTPUTS_DIR'], 'inventory.json').write_text(json.dumps(receipt))
    return 0


if __name__ == '__main__':
    sys.exit(main())
