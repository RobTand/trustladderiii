"""Harvest immutable PR graph pairs remotely and replay core revisions offline.

Both modes run on BuildBuddy. Harvest invokes only Bazel queries. Consecutive
ranges share immutable snapshots, so thirty merged ranges need 31 exports.
The compressed archive lets a later core revision reuse the same graph evidence.
"""
import argparse
import gzip
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess

import tarfile
import tempfile

import bazel_gate
import bazel_graph_impact as adapter
from child_env import child_env

OLD_CORE_SHA = '81fe2fec027f2f91aa433a10e8206904e38d86c2'


def reasons(errors):
    """Compact the repeated old-budget diagnostics without hiding a failure kind."""
    result = set()
    reaches = []
    for error in errors:
        reach = re.search(r' reach (\d+) exceeds budget (\d+)', error)
        if reach:
            reaches.append(tuple(map(int, reach.groups())))
        elif error.startswith(('u_pct ', 'loc_u_pct ')):
            reason = error.split(';', 1)[0].split(' (tolerance', 1)[0]
            result.add(reason + ('; gain unrecorded' if 'record the gain' in error else ''))
        else:
            result.add(error)
    if reaches:
        sizes = sorted({size for size, _ in reaches})
        result.add(f'{len(reaches)} tests reach {", ".join(map(str, sizes))} above global budget {reaches[0][1]}')
    return '<br>'.join(sorted(result)) or 'pass'


def report(result):
    rows = ['# TL3 merged-PR graph replay', '',
            f'Old rules fail {result["old_failed"]}/{result["ranges"]}; '
            f'revised rules fail {result["new_failed"]}/{result["ranges"]}.', '',
            f'Old core comes from TL3 `{result["old_core"]}`. '
            f'Revised core: `{result["current_core"]["source"]}`.', '',
            'Each range compares immutable base and head graphs. New rules use package base ceilings '
            'and typed edge differences. The JSON artifact retains every full diagnostic and edge.', '',
            '| PR | Old failure reasons | Revised failure reasons |', '|---|---|---|']
    rows += [f'| #{row["pr"]} | {reasons(row["old_errors"])} | {reasons(row["new_errors"])} |'
             for row in result['records']]
    return '\n'.join(rows) + '\n'


def harvest(ranges, output):
    cache = {}
    workspace = bazel_gate.Workspace(graph_cache=cache)
    original = workspace.run('git', 'rev-parse', 'HEAD').strip()
    directory = Path(tempfile.mkdtemp(prefix='tl3-graph-replay-'))
    (directory / 'snapshots').mkdir()
    records = []
    try:
        for row in ranges:
            base, head = row['base'], row['head']
            for sha in (base, head):
                workspace.ensure_commit(sha)
            workspace.prepare_diff(base, head)
            for sha in (base, head):
                snapshot = directory / 'snapshots' / f'{sha}.json.gz'
                if not snapshot.exists():
                    graph, deps, hashes, crates, _ = cache[sha]
                    tracked = set(workspace.run('git', 'ls-tree', '-r', '--name-only', sha).splitlines())
                    unexpected = {adapter.path(source) for source in graph['loc']} - tracked
                    if unexpected:
                        raise ValueError(f'{sha} exports sources absent from its Git tree: {sorted(unexpected)}')
                    data = {'graph': graph, 'deps': deps, 'hashes': hashes, 'crates': sorted(crates)}
                    with gzip.open(snapshot, 'wt') as handle:
                        json.dump(data, handle, sort_keys=True)
            changed, deleted = workspace.changes(base, head)
            records.append({**row, 'changed_files': sorted(changed | deleted)})
            (directory / 'ranges.json').write_text(json.dumps(records, indent=2) + '\n')
            print(f'HARVEST {len(records)}/{len(ranges)} PR #{row["pr"]}: {base[:12]}..{head[:12]}', flush=True)
    finally:
        workspace.checkout(original)
        with tarfile.open(output / 'graph-impact-snapshots.tar.gz', 'w:gz') as archive:
            archive.add(directory, arcname='graphs')
    return records


def old_core(root):
    file = root / 'old_graph_impact.py'
    file.write_bytes(subprocess.check_output(['git', 'show', f'{OLD_CORE_SHA}:tools/ci/graph_impact.py'], env=child_env()))
    spec = importlib.util.spec_from_file_location('tl3_graph_impact_before_review_1', file)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def replay(directory, output):
    legacy = old_core(output)
    records = []
    for row in json.loads((directory / 'ranges.json').read_text()):
        snapshots = []
        for key in ('base', 'head'):
            with gzip.open(directory / 'snapshots' / f'{row[key]}.json.gz', 'rt') as handle:
                snapshots.append(json.load(handle))
        base, head = [snapshot['graph'] for snapshot in snapshots]
        changed = adapter.changed_tests(base, head, snapshots[0]['hashes'], snapshots[1]['hashes'],
                                        set(row['changed_files']))
        prior_policy = legacy.baseline_record(base, tolerances={'u_pct': .1, 'loc_u_pct': .1, 'scc_max': 0})
        before = legacy.compare(base, head, prior_policy, changed_tests=changed)
        # Measurements retain their original data, while the adapter supplies
        # package ceilings and typed edge changes to the newly vendored core.
        after = adapter.check(base, head, changed, adapter.baseline(base))
        record = {**row, 'old_errors': before, 'new_errors': after['errors'],
                  'summary': after['summary'], 'edge_diff': after['edge_diff'],
                  'base_package_max_reach': after['base_package_max_reach']}
        records.append(record)
        print(f'REPLAY PR #{row["pr"]}: old {"FAIL" if before else "PASS"}, '
              f'new {"FAIL" if after["errors"] else "PASS"}; {after["summary"]}', flush=True)
        for error in after['errors']:
            print(f'REPLAY_FAILURE #{row["pr"]}: {error}', flush=True)
    result = {'old_core': OLD_CORE_SHA, 'current_core': json.loads(
                  (Path(__file__).parent / 'graph-impact-core.json').read_text()),
              'ranges': len(records), 'old_failed': sum(bool(row['old_errors']) for row in records),
              'new_failed': sum(bool(row['new_errors']) for row in records), 'records': records}
    (output / 'graph-impact-replay.json').write_text(json.dumps(result, indent=2, sort_keys=True) + '\n')
    (output / 'graph-impact-replay.md').write_text(report(result))
    print(f'REPLAY_TOTAL {len(records)} PRs, old failures {result["old_failed"]}, '
          f'new failures {result["new_failed"]}', flush=True)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--harvest', type=Path, help='JSON ranges with pr, base, head')
    mode.add_argument('--graphs', type=Path, help='extracted graphs directory from a harvest artifact')
    args = parser.parse_args()
    output = Path(os.environ['BUILDBUDDY_ARTIFACTS_DIRECTORY'])
    if args.harvest:
        records = harvest(json.loads(args.harvest.read_text()), output)
        return int(len(records) != len(json.loads(args.harvest.read_text())))
    replay(args.graphs, output)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
