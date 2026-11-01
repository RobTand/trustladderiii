"""Prove Bazel runs the tests the Cargo and Python gate would run on this tree.

`static` compares Bazel's test targets with `cargo metadata` and with the Python
suites tools/ci/check.py discovers, for whatever tree is checked out. It needs
cargo, git and bazel query, and runs on every CI build.

`receipts` compares test names: the per-test receipts Bazel's runners wrote
against rust_gate timing files from a Cargo run of the same commit.
"""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tomllib
import zipfile

import check

ROOT = Path(__file__).resolve().parents[2]
FIX = 'tools/bazel/generate.sh'
RUST_RUNNER = '//tools/bazel:rust_runner.py'
PYTHON_RUNNER = '//tools/bazel:python_runner.py'
# rust_gate.collect_binaries runs the controller's github binary as two groups.
GATE_GROUPS = {'tl3-controller/github': ['flow', 'notices']}
RUST_KINDS = {'lib', 'bin', 'test', 'example', 'bench'}


def python_modules(files):
    """The test modules check.py's pytest projects and unittest dirs collect."""
    paths = [Path(path) for path in files]
    projects = check.python_projects(paths)
    standalone = set(check.standalone_test_dirs(paths, projects))
    modules = []
    for path in paths:
        if not check.active(path) or not path.match('test_*.py'):
            continue
        owners = [project for project in projects if path.is_relative_to(project)]
        if owners:
            owner = max(owners, key=lambda project: len(project.parts))
            options = projects[owner].get('tool', {}).get('pytest', {}).get('ini_options', {})
            roots = options.get('testpaths', ['.'])
            if not any(path.is_relative_to(owner / root) for root in roots):
                continue
        elif path.parent not in standalone:
            continue
        modules.append(str(path))
    return sorted(modules)


def cargo_binaries(metadata, root=ROOT):
    """Map each binary rust_gate would run to (source file, harness kind)."""
    binaries = {}
    for package in metadata['packages']:
        manifest = Path(package['manifest_path'])
        declared = {test['name']: test.get('harness', True)
                    for test in tomllib.loads(manifest.read_text()).get('test', [])}
        for target in package['targets']:
            if not RUST_KINDS & set(target['kind']):
                continue
            source = os.path.relpath(target['src_path'], root)
            harness = 'libtest' if declared.get(target['name'], True) else 'custom'
            binary = f"{package['name']}/{target['name']}"
            for group in GATE_GROUPS.get(binary, ['']):
                binaries[binary + (f'-{group}' if group else '')] = (source, harness)
    return binaries


def bazel_tests(query_lines):
    """Read Rust binary ids and Python modules from `bazel query --output=streamed_jsonproto`."""
    rust, python = {}, []
    for line in query_lines:
        rule = json.loads(line).get('rule')
        if not rule:
            continue
        attributes = {attribute['name']: attribute for attribute in rule.get('attribute', [])}
        main = attributes.get('main', {}).get('stringValue')
        args = attributes.get('args', {}).get('stringListValue', [])
        if main == RUST_RUNNER:
            if args[1] in rust:
                raise RuntimeError(f'two Bazel targets run {args[1]}')
            rust[args[1]] = args[2]
        elif main == PYTHON_RUNNER:
            python.append(args[0])
    return rust, sorted(python)


def compare_static(cargo, bazel_rust, expected_python, bazel_python):
    problems = []
    for binary, (source, harness) in sorted(cargo.items()):
        if binary not in bazel_rust:
            problems.append(f'Bazel does not run {source} (Cargo binary {binary}).')
        elif bazel_rust[binary] != harness:
            problems.append(f'Bazel runs {source} with the {bazel_rust[binary]} harness; '
                            f'Cargo.toml says {harness}.')
    for binary in sorted(bazel_rust.keys() - cargo.keys()):
        problems.append(f'Bazel runs {binary}, which Cargo no longer builds.')
    for module in sorted(set(expected_python) - set(bazel_python)):
        problems.append(f'Bazel does not run {module}.')
    for module in sorted(set(bazel_python) - set(expected_python)):
        problems.append(f'Bazel runs {module}, which the Python gate does not collect.')
    if len(bazel_python) != len(set(bazel_python)):
        problems.append('Bazel runs a Python module twice.')
    if not cargo or not expected_python:
        problems.append('Discovery found no tests; refusing an empty inventory.')
    if problems:
        raise RuntimeError('Test parity drift:\n' + '\n'.join(problems) + f'\nFix: {FIX}')


def compare(expected, actual):
    """Name-level parity between Cargo and Bazel receipts."""
    problems = []
    for language in ['rust', 'python']:
        left, right = expected[language], actual[language]
        if language == 'python':
            left, right = {'suites': left}, {'suites': right}
        for binary in sorted(left.keys() | right.keys()):
            a, b = left.get(binary), right.get(binary)
            if a is None and b == [] and language == 'rust':
                continue  # Zero-test binaries; static parity proves they exist.
            if a is None or b is None:
                problems.append(f'{language} binary {binary}: missing from {"Cargo" if a is None else "Bazel"}')
                continue
            for names in [a, b]:
                if len(names) != len(set(names)):
                    problems.append(f'{binary}: duplicate tests')
            missing, extra = sorted(set(a) - set(b)), sorted(set(b) - set(a))
            if missing or extra:
                problems.append(f'{binary}: missing={missing}, extra={extra}')
    if problems:
        raise RuntimeError('Test parity drift:\n' + '\n'.join(problems) + f'\nFix: {FIX}')
    if not expected['rust'] and not expected['python']:
        raise RuntimeError('empty test inventory')


def custom_names(output):
    names = re.findall(r'^test (.+?) \.\.\. ok(?:[ ;(].*)?$', output, re.MULTILINE)
    counts = re.findall(r'^(\d+) .+ scenarios passed(?:;.*)?$', output, re.MULTILINE)
    if len(counts) != 1 or len(names) != int(counts[0]):
        raise RuntimeError('custom harness scenario count does not match its timing markers')
    if len(set(names)) != len(names):
        raise RuntimeError('duplicate custom scenario')
    return sorted(names)


def cargo_receipts(paths, files):
    binaries = {}
    for path in paths:
        for key in json.loads(path.read_text())['tests']:
            binary, name = key.split('::', 1)
            if name != '__tail__':
                binaries.setdefault(binary, []).append(name)
    return {'rust': binaries, 'python': python_modules(files)}


def read_receipt(outputs):
    """A test's inventory.json from its test.outputs directory, zipped or not."""
    plain, archive = outputs / 'inventory.json', outputs / 'outputs.zip'
    if plain.is_file():
        return json.loads(plain.read_text())
    if archive.is_file():
        with zipfile.ZipFile(archive) as zipped:
            if 'inventory.json' in zipped.namelist():
                return json.loads(zipped.read('inventory.json'))
    return None


def bazel_receipts(directory):
    inventory = {'rust': {}, 'python': []}
    for outputs in sorted(directory.rglob('test.outputs')):
        receipt = read_receipt(outputs)
        if receipt is None:
            continue
        for binary, names in receipt.get('rust', {}).items():
            if binary in inventory['rust']:
                raise RuntimeError(f'duplicate Bazel binary receipt: {binary}')
            inventory['rust'][binary] = names
        inventory['python'].extend(receipt.get('python', []))
    return inventory


def cargo_metadata():
    cargo = os.environ.get('CARGO') or shutil.which('cargo') or str(Path.home() / '.cargo/bin/cargo')
    output = subprocess.check_output([cargo, 'metadata', '--no-deps', '--format-version', '1',
                                      '--locked', '--manifest-path', str(ROOT / 'Cargo.toml')],
                                     cwd=ROOT)
    return json.loads(output)


def bazel_query(bazel):
    output = subprocess.check_output([*bazel, 'query', 'tests(//...)', '--output=streamed_jsonproto'],
                                     cwd=ROOT, text=True)
    return output.splitlines()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    static = commands.add_parser('static')
    static.add_argument('--bazel', default='bazel', help='bazel command, split on spaces')
    receipts = commands.add_parser('receipts')
    receipts.add_argument('--cargo-receipt', type=Path, action='append', required=True)
    receipts.add_argument('--bazel-testlogs', type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == 'static':
            rust, python = bazel_tests(bazel_query(args.bazel.split()))
            compare_static(cargo_binaries(cargo_metadata()), rust,
                           python_modules(check.tracked_files()), python)
            summary = {'rust_binaries': len(rust), 'python_modules': len(python)}
        else:
            actual = bazel_receipts(args.bazel_testlogs)
            compare(cargo_receipts(args.cargo_receipt, check.tracked_files()), actual)
            summary = {'rust_binaries': len(actual['rust']),
                       'rust_tests': sum(map(len, actual['rust'].values())),
                       'python_modules': len(actual['python'])}
    except RuntimeError as error:
        sys.exit(str(error))
    print(json.dumps(summary, sort_keys=True))


if __name__ == '__main__':
    main()
