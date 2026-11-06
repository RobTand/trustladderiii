"""Reproduce R7a evidence on a disposable BuildBuddy runner, never a fleet host."""
import argparse
import json
import os
import re
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[3]
GROUPS = ['dispatch', 'core', 'fence', 'approval', 'converges', 'once', 'dedup']
BASE = '45436ffd147eae63221403a5018a0d3f002617c9'


def run(*command, capture=False):
    result = subprocess.run(command, check=True, text=True,
                            stdout=subprocess.PIPE if capture else None)
    return result.stdout if capture else None


def bazel(*args):
    return run('bazel', *args)


def install_uv():
    sys.path.insert(0, str(ROOT / 'tools/ci'))
    from remote_gate import INSTALL_UV
    run('bash', '-c', INSTALL_UV)
    os.environ['PATH'] = str(Path.home() / '.local/bin') + ':' + os.environ['PATH']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['isolated', 'baseline', 'controller', 'checks', 'features', 'graph'])
    parser.add_argument('--base', default=BASE)
    args = parser.parse_args()
    if args.mode == 'isolated':
        for group in GROUPS:
            bazel('test', '--config=remote', '--jobs=1', '--nocache_test_results', '//crates/delivery:verus_' + group)
    elif args.mode == 'baseline':
        # Restore the exact pre-R7a production and proof bytes in this disposable
        # runner. Retain the measurement adapter, and print the source SHA.
        print(json.dumps({'baseline_source': args.base}), flush=True)
        paths = ['crates/delivery/src/step.rs'] + ['crates/delivery/proofs/' + g + '.rs' for g in GROUPS if g != 'dispatch']
        paths += ['tools/bazel/verus.bzl']
        saved = {path: (ROOT / path).read_bytes() for path in paths}
        try:
            for path in paths[:-1]:
                (ROOT / path).write_text(run('git', 'show', args.base + ':' + path, capture=True))
            path = ROOT / paths[-1]
            text = path.read_text()
            start = text.index('if native.package_name() == "crates/delivery":')
            old_counts = {"dispatch": 41, "core": 18, "once": 39, "converges": 73, "dedup": 42, "approval": 79, "fence": 50}
            text = text[:start] + re.sub(r'for name, count in \{[^\n]*\}.items\(\):',
                'for name, count in ' + repr(old_counts) + '.items():', text[start:], count=1)
            text = text.replace('srcs = ["src/step.rs"] +', 'srcs = ["src/step.rs", "src/mirror.rs"] +')
            path.write_text(text)
            bazel('test', '--config=remote', '--jobs=80', '--nocache_test_results',
                  *['//crates/delivery:verus_' + g for g in ['core', 'fence', 'approval', 'converges']])
        finally:
            for path, data in saved.items():
                (ROOT / path).write_bytes(data)
    elif args.mode == 'controller':
        # Compile every Rust target that depends on the delivery library. No
        # controller source is changed, and no giant test suite is run here.
        labels = run('bazel', 'query', 'kind("rust_(test|binary|library) rule", rdeps(//crates/controller:all, //crates/delivery:tl3_delivery))', capture=True).splitlines()
        if not labels:
            raise RuntimeError('no controller dependants selected')
        print(json.dumps({'controller_compile_targets': len(labels)}), flush=True)
        bazel('build', '--config=remote', '--jobs=80', *labels)
    elif args.mode == 'features':
        bazel('run', '--config=remote', '@rules_rust//tools/upstream_wrapper:cargo', '--',
              'test', '-p', 'tl3-delivery', '--locked', '--no-default-features', '--all-targets', '--no-run')
    elif args.mode == 'checks':
        install_uv()
        with tempfile.TemporaryDirectory(prefix='delivery-cargo-') as temporary:
            cargo = Path(temporary, 'cargo')
            cargo.write_text('#!/bin/sh\nexec bazel run --config=remote @rules_rust//tools/upstream_wrapper:cargo -- "$@"\n')
            cargo.chmod(0o755)
            os.environ['CARGO'] = str(cargo)
            run(str(cargo), 'metadata', '--locked', '--no-deps', '--format-version=1', capture=True)
            run('python3', 'tools/ci/bazel_generated.py')
        run('uv', 'tool', 'run', '--from', 'ruff==0.14.14', 'ruff', 'check', '--isolated', '--select', 'E9,F',
            'crates/delivery/tools/verus_runner.py', 'crates/delivery/tools/evidence.py')
    else:
        run('python3', 'tools/ci/graph_impact_check.py', '--base', args.base)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
