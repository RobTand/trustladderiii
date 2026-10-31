"""Hosted CI planning and result aggregation. Development tests use PrismaBuild."""
import argparse
import json
import os
from pathlib import Path

import check
import shards

RUNNERS = ['ubuntu-24.04-arm', 'ubuntu-24.04']


def plan(total):
    if not 4 <= total <= 18:
        raise ValueError('Use 4 through 18 test jobs, leaving capacity for lint and aggregation')
    result = {}
    for language, count in [('rust', total - 2), ('python', 2)]:
        result[language] = {'include': [
            {'index': index, 'total': count, 'runner': RUNNERS[index % 2]}
            for index in range(count)
        ]}
    groups = python_partition(check.tracked_files(), 2)
    for entry in result['python']['include']:
        entry['deploy_helper'] = 'unittest:deploy/dl380g10' in groups[entry['index']]
    return result


def aggregate(results):
    if not results or any(value.get('result') != 'success' for value in results.values()):
        raise RuntimeError(f'Every prerequisite must succeed: {results}')


def python_partition(files, total):
    projects = check.python_projects(files)
    names = [f'project:{directory}' for directory in projects]
    names.extend(f'unittest:{directory}' for directory in check.standalone_test_dirs(files, projects))
    return shards.partition(names, total, shards.weights('python'))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('plan').add_argument('--jobs', type=int, required=True)
    commands.add_parser('python').add_argument('--shard', type=shards.parse_shard, required=True)
    commands.add_parser('aggregate')
    args = parser.parse_args()
    if args.command == 'plan':
        result = plan(args.jobs)
        print(json.dumps(result), flush=True)
        if 'GITHUB_OUTPUT' in os.environ:
            with Path(os.environ['GITHUB_OUTPUT']).open('a') as output:
                for language, matrix in result.items():
                    output.write(f'{language}={json.dumps(matrix)}\n')
    elif args.command == 'aggregate':
        aggregate(json.loads(os.environ['NEEDS_JSON']))
    else:
        index, total = args.shard
        files = check.tracked_files()
        assigned = python_partition(files, total)[index]
        if not assigned:
            raise RuntimeError('Empty Python shard')
        print(json.dumps({'python_shard': index, 'total': total, 'suites': assigned}), flush=True)
        check.python(files, selected=set(assigned), lint=index == 0)


if __name__ == '__main__':
    main()
