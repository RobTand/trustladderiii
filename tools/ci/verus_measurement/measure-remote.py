"""Exact-source, disposable Verus measurement preparation and child collection."""
from __future__ import annotations

import argparse
from contextlib import contextmanager
from dataclasses import asdict, dataclass
import difflib
import hashlib
import io
import json
import math
import os
from pathlib import Path
import re
import stat
import subprocess
import xml.etree.ElementTree as ET
import zipfile

RUNNER = 'crates/verus-runner/src/bin/verus-runner.rs'
INVENTORY = 'tools/bazel/verus.bzl'


def require(condition, message):
    if not condition:
        raise ValueError(message)


def pairs(items):
    result = {}
    for key, value in items:
        require(key not in result, f'duplicate JSON key: {key}')
        result[key] = value
    return result


def loads(text):
    return json.loads(text, object_pairs_hook=pairs,
                      parse_constant=lambda value: require(False, 'nonfinite JSON number'))


def exact_keys(value, keys):
    require(type(value) is dict and set(value) == set(keys), f'expected fields: {keys}')


def digest(data):
    return hashlib.sha256(data).hexdigest()


def encoded(value):
    return (json.dumps(value, sort_keys=True, indent=2, allow_nan=False) + '\n').encode()


def regular_bytes(path):
    # O_NOFOLLOW also closes the lstat/open symlink race. Never alter source modes.
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as stream:
        require(stat.S_ISREG(os.fstat(stream.fileno()).st_mode), f'not a regular file: {path}')
        return stream.read()


def put(directory, name, data):
    require(Path(name).name == name and name not in ('', '.', '..'), 'unsafe artifact name')
    path = directory / name
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'wb') as stream:
        stream.write(data)
    return {'name': name, 'sha256': digest(data), 'bytes': len(data)}


def owned_directory(path):
    path.mkdir(mode=0o700, parents=False, exist_ok=False)
    return path


@dataclass(frozen=True)
class Selection:
    target: str
    selector: str
    expected: int


@dataclass(frozen=True)
class Config:
    head: str
    comparison_base: str
    branch: str
    mode: str
    watchdog_seconds: int
    outer_seconds: int
    coordinator_minutes: int
    diagnostic_authorization: str | None
    raw_queries: bool
    selections: tuple[Selection, ...]

    @classmethod
    def parse(cls, value):
        exact_keys(value, cls.__dataclass_fields__)
        for key in ('head', 'comparison_base'):
            require(type(value[key]) is str and re.fullmatch('[0-9a-f]{40}', value[key]),
                    f'{key} must be an immutable lowercase SHA40')
        require(type(value['branch']) is str and re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9/_.-]*', value['branch']), 'invalid branch')
        require(value['mode'] in ('normal', 'diagnostic'), 'unknown timing mode')
        for key in ('watchdog_seconds', 'outer_seconds', 'coordinator_minutes'):
            require(type(value[key]) is int and value[key] > 0, f'{key} must be a positive integer')
        require(value['outer_seconds'] == 180, 'outer timeout must be explicitly 180 seconds')
        require(value['watchdog_seconds'] == (45 if value['mode'] == 'normal' else 120), 'mode/watchdog mismatch')
        auth = value['diagnostic_authorization']
        require((value['mode'] == 'normal' and auth is None) or
                (value['mode'] == 'diagnostic' and type(auth) is str and 0 < len(auth) <= 200),
                'diagnostic mode requires an explicit authorization reference; normal requires null')
        require(type(value['raw_queries']) is bool, 'raw_queries must be explicit boolean')
        require(type(value['selections']) is list and value['selections'], 'empty selection manifest')
        rows = []
        for row in value['selections']:
            exact_keys(row, Selection.__dataclass_fields__)
            require(type(row['target']) is str and re.fullmatch(r'//crates/kernel:verus_[a-z0-9_]+', row['target']), 'unsupported target')
            require(type(row['selector']) is str and re.fullmatch(r'[A-Za-z0-9_:]+(?:#[A-Za-z0-9_:]+)?', row['selector']), 'invalid selector')
            require(type(row['expected']) is int and row['expected'] > 0, 'expected must be a positive integer')
            rows.append(Selection(**row))
        require(len({r.target for r in rows}) == len(rows) == len({r.selector for r in rows}), 'target/selector bijection violated')
        return cls(**{**value, 'selections': tuple(rows)})

    def json(self):
        return loads(encoded(asdict(self)))

    def command(self):
        return ['bazel', 'test', '--config=remote', '--config=ci', '--jobs=16',
                f'--test_timeout={self.outer_seconds}', '--keep_going',
                '--noremote_accept_cached', '--nocache_test_results',
                *[s.target for s in self.selections]]


def git(root, *args):
    return subprocess.check_output(['git', '-C', str(root), *args])


def source_identity(root, config):
    require(git(root, 'rev-parse', 'HEAD').decode().strip() == config.head, 'checkout HEAD differs from config head')
    require(not git(root, 'status', '--porcelain', '--untracked-files=all'), 'source fixture must be clean, including untracked files')
    files = {}
    for name in git(root, 'ls-files', '-z').decode().split('\0'):
        if name:
            path = root / name
            require(not any(p.is_symlink() for p in path.parents if p != root.parent), 'source directory symlink')
            data = regular_bytes(path)
            files[name] = {'sha256': digest(data), 'bytes': len(data)}
    return {'head': config.head, 'comparison_base': config.comparison_base,
            'baseline_measured': False, 'history_required': False,
            'base_object_required': False, 'files': files}


def replace_once(source, before, after):
    require(source.count(before) == 1, f'transformation anchor count differs from one: {before[:100]}')
    return source.replace(before, after)


def instrument(source, config):
    require('--no-verify' not in source and '--log-all' not in source, 'unexpected verifier mode')
    for pin in ('"--num-threads",\n                "1",', '"--rlimit",\n                "30",',
                '.env("OMP_NUM_THREADS", "1")', '.env("RAYON_NUM_THREADS", "1")',
                '.env("VERUS_Z3_PATH", &self.z3)', 'process.args(["--time", "--output-json"]);'):
        require(source.count(pin) == 1, f'missing or duplicate pinned setting: {pin}')
    source = replace_once(source, '.args(["--kill-after=5s", "45s"])',
                          f'.args(["--kill-after=5s", "{config.watchdog_seconds}s"])')
    source = replace_once(source, 'verdict["success"].as_bool() != Some(true)',
                          'verdict.get("success").is_some_and(|v| v.as_bool() != Some(true))')
    source = replace_once(source, '    let result = toolchain.verify(source, module, measured)?;',
                          '    let measured = true;\n    let result = toolchain.verify(source, module, measured)?;')
    source = replace_once(source, '''            return Err(format!(
                "{label}: group exceeds the hard 20s ceiling ({seconds:.3}s)"
            ));''', '''            eprintln!("VERUS_WALL_WARNING {label}: exceeds historical 20s ceiling ({seconds:.3}s)");''')
    if config.raw_queries:
        source = replace_once(source, '        let result = process\n', '''        if let Some(directory) = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR") {
            let logs = Path::new(&directory).join("raw-queries");
            std::fs::create_dir_all(&logs)?;
            process.arg("--log-all").arg("--log-dir").arg(logs);
        }
        let result = process
''')
    return source


def prepare(root, config, output):
    identity = source_identity(root, config)
    inventory = regular_bytes(root / INVENTORY).decode()
    # The supported inventory is the kernel groups table, not an inferred RP campaign.
    start = inventory.index('        groups = {', inventory.index('native.package_name() == "crates/kernel"'))
    end = inventory.index('        for name, (module, count) in groups.items():', start)
    entries = re.findall(r'"([a-z0-9_]+)": \("([^"]+)", ([0-9]+)\)', inventory[start:end])
    require(len(entries) == len({g for g, _, _ in entries}), 'duplicate inventory target')
    table = {f'//crates/kernel:verus_{g}': (s, int(n)) for g, s, n in entries}
    for row in config.selections:
        require(table.get(row.target) == (row.selector, row.expected), f'selector/count differs from exact source: {row.target}')
    loop = inventory[end:inventory.index('\n    if native.package_name()', end)]
    for anchor in ('        for name, (module, count) in groups.items():',
                   '                name = "verus_" + name,',
                   '                module = module,',
                   '                expected_obligations = count,'):
        require(loop.count(anchor) == 1, 'inventory generation anchor missing or repeated')
    original = regular_bytes(root / RUNNER)
    changed = instrument(original.decode(), config).encode()
    patch = ''.join(difflib.unified_diff(original.decode().splitlines(True), changed.decode().splitlines(True),
                                       fromfile='a/' + RUNNER, tofile='b/' + RUNNER)).encode()
    manifest = []
    for name, data in [('source.json', encoded(identity)), ('config.json', encoded(config.json())),
                       ('selections.json', encoded([asdict(s) for s in config.selections])),
                       ('runner.patch', patch), ('runner-preview.rs', changed),
                       ('command.json', encoded(config.command()))]:
        manifest.append(put(output, name, data))
    result = {'source_sha256': digest(encoded(identity)), 'config_sha256': digest(encoded(config.json())),
              'selection_sha256': digest(encoded([asdict(s) for s in config.selections])),
              'instrumentation_sha256': digest(patch), 'runner_sha256': digest(changed),
              'bazel_argv': config.command(), 'preparation': 'PASS', 'build_executed': False,
              'artifacts': manifest}
    put(output, 'preparation.json', encoded(result))
    return result, original, changed


@contextmanager
def disposable_runner(root, original, changed):
    path = root / RUNNER
    written = False
    try:
        require(regular_bytes(path) == original, 'runner changed after preparation')
        written = True
        path.write_bytes(changed)
        yield
    finally:
        if written:
            path.write_bytes(original)
            require(regular_bytes(path) == original, 'source restoration failed')


def metric(value):
    return value if type(value) is int and value >= 0 else None


def child_verdict(stats, proof, measurement, row):
    require(all(type(value) is dict for value in (stats, proof, measurement)), 'raw receipt must be a JSON object')
    verdict = stats.get('verification-results', {})
    require(type(verdict) is dict, 'invalid verdict object')
    require(type(verdict.get('verified')) is int and verdict['verified'] == row.expected and
            type(verdict.get('errors')) is int and verdict['errors'] == 0 and
            verdict.get('encountered-error') is False and verdict.get('encountered-vir-error') is False and
            verdict.get('is-verifying-entire-crate') is False and
            ('success' not in verdict or verdict['success'] is True), 'child verdict/count/selection failed')
    require(type(proof.get('proofs')) is dict and set(proof['proofs']) == {row.target}, 'wrong/duplicate proof row')
    p = proof['proofs'][row.target]
    require(type(p) is dict, 'invalid proof row')
    require(p.get('kind') == 'verus' and p.get('module') == row.selector and
            type(p.get('verified')) is int and p['verified'] == row.expected and
            type(p.get('expected_obligations')) is int and p['expected_obligations'] == row.expected and
            type(p.get('threads')) is int and p['threads'] == 1, 'proof selection/count/thread mismatch')
    wall = p.get('seconds')
    require(type(wall) in (int, float) and math.isfinite(wall) and wall >= 0, 'missing wall time')
    times = stats.get('times-ms', {})
    require(type(times) is dict, 'invalid times-ms object')
    smt = times.get('smt', {})
    require(type(smt) is dict, 'invalid SMT metrics object')
    resources = {'rlimit_run': metric(smt.get('rlimit-run')), 'rlimit_init': metric(smt.get('rlimit-init')),
                 'total_ms': metric(times.get('total')), 'threads': metric(times.get('num-threads'))}
    require(measurement.get('label') == row.target and type(measurement.get('verified')) is int and measurement['verified'] == row.expected and
            measurement.get('seconds') == wall and measurement.get('rlimit_budget') == 30,
            'measurement identity/budget mismatch')
    require(measurement.get('rlimit_run') == resources['rlimit_run'] and
            measurement.get('rlimit_init') == resources['rlimit_init'], 'measurement resource mismatch')
    return {'proof': 'PASS', 'resources': 'PASS' if None not in resources.values() and resources['threads'] == 1 else 'UNKNOWN',
            **resources, 'wall_seconds': wall, 'wall_warning': wall > 20}


def check_test_xml(data):
    root = ET.fromstring(data)
    suites = [root] if root.tag == 'testsuite' else list(root.findall('testsuite'))
    require(len(suites) == 1 and suites[0].get('tests') == '1' and
            suites[0].get('failures') == '0' and suites[0].get('errors') == '0' and
            len(suites[0].findall('testcase')) == 1 and
            not any(node.tag in ('failure', 'error', 'skipped') for node in root.iter()),
            'selected child XML does not report exactly one passing test')


def output_bytes(directory, name):
    direct = directory / 'test.outputs' / name
    if direct.exists() or direct.is_symlink():
        require(not direct.parent.is_symlink(), 'unexpected output directory symlink')
        return regular_bytes(direct)
    archive = directory / 'test.outputs' / 'outputs.zip'
    require(not archive.parent.is_symlink(), 'unexpected output directory symlink')
    with zipfile.ZipFile(io.BytesIO(regular_bytes(archive))) as zipped:
        matches = [info for info in zipped.infolist() if info.filename == name]
        require(len(matches) == 1, f'missing/duplicate curated output {name}')
        info = matches[0]
        mode = info.external_attr >> 16
        require(stat.S_IFMT(mode) in (0, stat.S_IFREG) and info.file_size <= 16 * 1024 * 1024, 'unsafe zip member')
        return zipped.read(info)


def stage_tree(source, output, prefix, manifest):
    require(not source.is_symlink() and source.is_dir(), 'unsafe raw query directory')
    for path in sorted(source.iterdir()):
        require(not path.is_symlink(), 'raw query symlink refused')
        if path.is_dir():
            stage_tree(path, output, prefix + path.name + '__', manifest)
        else:
            manifest.append(put(output, prefix + path.name, regular_bytes(path)))


def collect(root, config, output, batch_exit):
    rows, files = [], []
    for index, selection in enumerate(config.selections):
        # Bazel itself owns the testlogs link. Resolve that one boundary only.
        directory = (root / 'bazel-testlogs').resolve(strict=True)
        for component in selection.target[2:].replace(':', '/').split('/'):
            directory = directory / component
            require(not directory.is_symlink() and directory.is_dir(), 'unexpected testlog directory link/type')
        for name in ('test.log', 'test.xml'):
            data = regular_bytes(directory / name)
            require(data, 'missing/empty selected child log/XML')
            if name == 'test.xml':
                check_test_xml(data)
            files.append(put(output, f'{index}-{name}', data))
        objects = {}
        for name in ('inventory.json', 'measurement.json', 'verifier.json'):
            data = output_bytes(directory, name)
            files.append(put(output, f'{index}-{name}', data))
            objects[name] = loads(data)
        rows.append({**asdict(selection), **child_verdict(objects['verifier.json'], objects['inventory.json'], objects['measurement.json'], selection)})
        if config.raw_queries:
            stage_tree(directory / 'test.outputs' / 'raw-queries', output, f'{index}-query-', files)
    run_values = [r['rlimit_run'] for r in rows]
    return {'child': 'PASS' if batch_exit == 0 else 'FAIL', 'rows': rows,
            'resources': 'PASS' if all(r['resources'] == 'PASS' for r in rows) else 'UNKNOWN',
            'rlimit_run_sum': sum(run_values) if None not in run_values else None,
            'init_included_in_sum': False, 'collection': 'PASS', 'artifacts': files}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config-json', required=True)
    parser.add_argument('--source', type=Path, default=Path.cwd())
    parser.add_argument('--output', type=Path, required=True, help='new owned directory')
    parser.add_argument('--dry-run', action='store_true')
    args = parser.parse_args()
    config = Config.parse(loads(args.config_json))
    output = owned_directory(args.output)
    prepared, original, changed = prepare(args.source, config, output)
    if args.dry_run:
        print(json.dumps(prepared))
        return
    summary = {'preparation': 'PASS', 'restoration': 'UNKNOWN', 'collection': 'UNKNOWN',
               'child': 'UNKNOWN', 'resources': 'UNKNOWN', 'overall': 'UNKNOWN',
               'parent': 'UNKNOWN', 'scanner': 'UNKNOWN', 'cleanup': 'UNKNOWN',
               'command_artifact_namespace': 'verus-measurement',
               'reserved_name_collision': False}
    try:
        reserved = output.parent / 'bbrun-wrapper-result.v1.json'
        require(not reserved.exists() and not reserved.is_symlink(), 'reserved wrapper artifact collision before command')
        with disposable_runner(args.source, original, changed):
            run = subprocess.run(config.command(), cwd=args.source, check=False)
            summary['child'] = 'PASS' if run.returncode == 0 else 'FAIL'
            summary.update(collect(args.source, config, output, run.returncode))
    except Exception:
        summary['collection'] = 'FAIL'
        raise
    finally:
        summary['reserved_name_collision'] = reserved.exists() or reserved.is_symlink()
        summary['restoration'] = 'PASS' if regular_bytes(args.source / RUNNER) == original else 'FAIL'
        # Parent/scanner/wrapper can only be observed after this command terminates.
        put(output, 'measurement-summary.json', encoded(summary))
        all_files = [{'name': p.name, 'sha256': digest(regular_bytes(p)), 'bytes': p.stat().st_size}
                     for p in sorted(output.iterdir())]
        put(output, 'artifact-manifest.json', encoded(all_files))
    require(all(summary.get(s) == 'PASS' for s in ('child', 'resources', 'collection', 'restoration')), 'measurement stages did not pass')


if __name__ == '__main__':
    main()
