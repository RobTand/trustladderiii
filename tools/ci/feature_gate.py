"""Compile feature-gated test and proof targets without running them."""
import argparse
from itertools import combinations
import json
from pathlib import Path
import subprocess
import time


def commands(metadata, mode):
    if mode not in {'features', 'kani'}:
        raise ValueError('Unknown compile mode')
    result = []
    packages = [package for package in metadata['packages']
                if package['id'] in metadata['workspace_members']
                and (mode != 'kani' or package['name'] == 'tl3-kernel')]
    if not packages:
        raise RuntimeError('No compile targets found')
    for package in sorted(packages, key=lambda package: package['name']):
        features = sorted(package['features'])
        for size in range(len(features) + 1):
            for selected in combinations(features, size):
                command = ['cargo', 'kani' if mode == 'kani' else 'test', '-p', package['name'],
                           '--no-default-features']
                if mode == 'kani':
                    command.append('--only-codegen')
                else:
                    # The controller deliberately rejects Loom outside library unit tests.
                    target = '--lib' if package['name'] == 'tl3-controller' and 'loom' in selected else '--all-targets'
                    command.extend(['--locked', '--no-run', target])
                if selected:
                    command.extend(['--features', ','.join(selected)])
                result.append(command)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['features', 'kani'])
    args = parser.parse_args()
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--no-deps', '--locked', '--format-version=1'], text=True))
    timings = []
    started = time.monotonic()
    try:
        for command in commands(metadata, args.mode):
            print(json.dumps({'compile_only': command}), flush=True)
            before = time.monotonic()
            result = subprocess.run(command, check=False)
            timings.append({'command': command, 'seconds': time.monotonic() - before,
                            'returncode': result.returncode})
            result.check_returncode()
    finally:
        report = {'mode': args.mode, 'seconds': time.monotonic() - started, 'builds': timings}
        path = Path(f'target/ci-feature-times-{args.mode}.json')
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report), flush=True)


if __name__ == '__main__':
    main()
