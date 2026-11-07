"""Exact-source, production-backed Verus measurement preparation and child collection."""
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
import sys
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
        require(value['mode'] in ('frontend-only', 'normal', 'diagnostic'), 'unknown timing mode')
        for key in ('watchdog_seconds', 'outer_seconds', 'coordinator_minutes'):
            require(type(value[key]) is int and value[key] > 0, f'{key} must be a positive integer')
        require(value['outer_seconds'] == 180, 'outer timeout must be explicitly 180 seconds')
        require(value['watchdog_seconds'] == (120 if value['mode'] == 'diagnostic' else 45), 'mode/watchdog mismatch')
        auth = value['diagnostic_authorization']
        require((value['mode'] != 'diagnostic' and auth is None) or
                (value['mode'] == 'diagnostic' and type(auth) is str and 0 < len(auth) <= 200),
                'diagnostic mode requires an explicit authorization reference; normal requires null')
        require(value['mode'] != 'frontend-only' or value['raw_queries'] is False, 'frontend cannot log queries')
        require(type(value['raw_queries']) is bool, 'raw_queries must be explicit boolean')
        require(type(value['selections']) is list and value['selections'], 'empty selection manifest')
        rows = []
        for row in value['selections']:
            exact_keys(row, Selection.__dataclass_fields__)
            require(type(row['target']) is str and re.fullmatch(r'//crates/kernel:verus_[a-z0-9_]+', row['target']), 'unsupported target')
            require(type(row['selector']) is str and re.fullmatch(r'[A-Za-z0-9_:]+(?:#(?:\*::)?[A-Za-z0-9_:]+)?', row['selector']), 'invalid selector')
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
                f'--test_env=TL3_VERUS_MODE={self.mode}',
                f'--test_env=TL3_VERUS_RAW_QUERIES={int(self.raw_queries)}',
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
            'tree': git(root, 'rev-parse', 'HEAD^{tree}').decode().strip(),
            'baseline_measured': False, 'history_required': False,
            'base_object_required': False, 'files': files}


def replace_once(source, before, after):
    require(source.count(before) == 1, f'transformation anchor count differs from one: {before[:100]}')
    return source.replace(before, after)



CAPABILITY = 'tl3.verus-runner.modes.v1'


def instrument(source, config):
    # Kept as the fixture/public preparation API. Modern paths never patch source.
    require(CAPABILITY in source, 'unsupported runner capability; update the measured source explicitly, never inject a runner')
    return source


def tooling_identity():
    return {'driver_sha256': globals().get('__driver_sha256__') or digest(regular_bytes(Path(__file__))),
            'runner_protocol': CAPABILITY}


def source_pins(root):
    text = regular_bytes(root / 'tools/bazel/verus_pins.bzl').decode()
    def pin(name):
        found = re.findall(r'^' + name + r' = "([^"\n]+)"$', text, re.M)
        require(len(found) == 1, 'missing/ambiguous source toolchain pin: ' + name)
        return found[0]
    version = pin('VERUS_VERSION')
    components = dict(re.findall(r'^    "(rustc|rustc-dev|rust-std)": "([0-9a-f]{64})",$', text, re.M))
    require(len(components) == 3, 'missing source rust component pins')
    return {'verus': version, 'url': f'https://github.com/verus-lang/verus/releases/download/release/{version}/verus-{version}-x86-linux.zip',
            'sha256': pin('VERUS_SHA256'), 'z3': pin('Z3_SHA256'), 'rust': pin('RUST_VERSION'),
            'rust_date': pin('RUST_DATE'), 'components': components}


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
                       ('command.json', encoded(config.command())),
                       ('tooling.json', encoded(tooling_identity())), ('pins.json', encoded(source_pins(root)))]:
        manifest.append(put(output, name, data))
    result = {'source_sha256': digest(encoded(identity)), 'config_sha256': digest(encoded(config.json())),
              'selection_sha256': digest(encoded([asdict(s) for s in config.selections])),
              'instrumentation_sha256': digest(patch), 'runner_sha256': digest(changed), 'pins_sha256': digest(encoded(source_pins(root))),
              'bazel_argv': config.command(), 'preparation': 'PASS', 'build_executed': False,
              'artifacts': manifest}
    put(output, 'preparation.json', encoded(result))
    return result, original, changed


@contextmanager
def disposable_runner(root, original, changed):
    path = root / RUNNER
    require(changed == original, 'source instrumentation is unsupported')
    try:
        require(regular_bytes(path) == original, 'runner changed after preparation')
        yield
    finally:
        require(regular_bytes(path) == original, 'source restoration failed')


def metric(value):
    return value if type(value) is int and value >= 0 else None


def child_verdict(stats, proof, measurement, row, mode="normal", execution=None, raw_queries=False):
    require(all(type(value) is dict for value in (stats, proof, measurement)), 'raw receipt must be a JSON object')
    if mode == 'frontend-only':
        return frontend_verdict(stats, proof, measurement, row, execution)
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
    functions = []
    modules = smt.get('smt-run-module-times')
    if type(modules) is list:
        for module in modules:
            require(type(module) is dict and type(module.get('function-breakdown')) is list, 'missing expanded function breakdown')
            for function in module['function-breakdown']:
                require(type(function) is dict and metric(function.get('rlimit')) is not None and function.get('success') is True, 'invalid expanded function metric')
                functions.append(function['rlimit'])
    require(functions and sum(functions) == resources['rlimit_run'] and
            measurement.get('function_rlimit_sum') == sum(functions) and
            measurement.get('function_count') == len(functions) and
            measurement.get('max_function_rlimit') == max(functions), 'expanded/raw resource disagreement')
    validate_execution(execution, row, mode, raw_queries)
    return {'proof': 'PASS', 'function_rlimit_sum': sum(functions), 'function_count': len(functions), 'resources': 'PASS' if None not in resources.values() and resources['threads'] == 1 else 'UNKNOWN',
            **resources, 'wall_seconds': wall, 'wall_warning': wall > 20}


def validate_execution(execution, row, mode, raw_queries=False):
    require(type(execution) is dict and execution.get('protocol') == CAPABILITY and
            execution.get('mode') == mode and type(execution.get('status')) is int and execution['status'] == 0,
            'missing/failed runner execution identity')
    argv = execution.get('argv')
    require(type(argv) is list and all(type(a) is str for a in argv), 'invalid actual argv')
    require(argv[:3] == ['/usr/bin/timeout', '--kill-after=5s', '120s' if mode == 'diagnostic' else '45s'], 'watchdog mismatch')
    for flag, value in (('--num-threads', '1'), ('--rlimit', '30'), ('--multiple-errors', '0'), ('-V', 'spinoff-all'), ('--edition', '2024')):
        require(argv.count(flag) == 1 and argv[argv.index(flag) + 1] == value, 'runner argv pin mismatch')
    require(argv.count('--time-expanded') == argv.count('--output-json') == 1 and
            '--build-sst' not in argv and '--profile' not in argv and '--profile-all' not in argv and
            argv.count('--no-verify') == int(mode == 'frontend-only'), 'mode argv mismatch')
    require(argv.count('--log-all') == argv.count('--log-dir') == int(raw_queries), 'raw query mode mismatch')
    owner, _, function = row.selector.partition('#')
    require(argv.count('--verify-only-module') == 1 and argv[argv.index('--verify-only-module') + 1] == owner,
            'argv selection mismatch')
    require(argv.count('--verify-function') == int(bool(function)) and
            (not function or argv[argv.index('--verify-function') + 1] == function), 'argv function mismatch')
    tools = execution.get('tools')
    exact_keys(tools, ('verifier', 'rustc', 'z3', 'rust_verify'))
    for tool in tools.values():
        exact_keys(tool, ('path', 'sha256', 'bytes'))
        require(type(tool['path']) is str and tool['path'].startswith('/') and
                type(tool['sha256']) is str and re.fullmatch('[0-9a-f]{64}', tool['sha256']) and
                type(tool['bytes']) is int and tool['bytes'] > 0, 'invalid pinned tool identity')
    require(type(execution.get('pins')) is dict and tools['z3']['sha256'] == execution['pins'].get('z3'), 'runtime Z3 pin mismatch')
    require(argv[3] == tools['verifier']['path'], 'actual verifier differs from tool identity')
    sysroot = str(Path(tools['rustc']['path']).parent.parent)
    require(argv.count('crates/kernel/verus/progress.rs') == 1, 'actual source selection mismatch')
    require(execution.get('sysroot') == sysroot and argv.count('--sysroot') == 1 and
            argv[argv.index('--sysroot') + 1] == sysroot, 'rustc executable/sysroot mismatch')
    require(execution.get('environment') == {'PATH': sysroot + '/bin:/usr/bin:/bin',
        'LD_LIBRARY_PATH': sysroot + '/lib', 'VERUS_USE_RUSTUP': '0', 'VERUS_Z3_PATH': tools['z3']['path'],
        'OMP_NUM_THREADS': '1', 'RAYON_NUM_THREADS': '1'}, 'production environment mismatch')
    return execution


def frontend_verdict(stats, inventory, measurement, row, execution):
    validate_execution(execution, row, 'frontend-only')
    v = stats.get('verification-results', {})
    require(type(v) is dict and type(v.get('verified')) is int and v['verified'] == 0 and
            type(v.get('errors')) is int and v['errors'] == 0 and
            v.get('encountered-error') is False and v.get('encountered-vir-error') is False and
            v.get('is-verifying-entire-crate') is False and ('success' not in v or v['success'] is True),
            'frontend verdict/error/selection failed')
    times = stats.get('times-ms', {})
    require(type(times) is dict and metric(times.get('total')) is not None and metric(times.get('num-threads')) == 1, 'missing frontend timing/configured-thread evidence')
    smt = times.get('smt', {})
    require(all(metric(smt.get(k)) == 0 for k in ('total', 'smt-init', 'smt-run', 'rlimit-init', 'rlimit-run')) and
            all(smt.get(k) == [] for k in ('smt-init-module-times', 'smt-run-module-times')), 'missing/conflicting no-SMT evidence')
    require(type(inventory.get('proofs')) is dict and set(inventory['proofs']) == {row.target}, 'wrong frontend inventory')
    p = inventory['proofs'][row.target]
    require(p.get('kind') == 'FRONTEND_ONLY' and p.get('module') == row.selector and
            type(p.get('expected_obligations')) is int and p['expected_obligations'] == row.expected and
            type(p.get('verified')) is int and p['verified'] == 0 and type(p.get('threads')) is int and p['threads'] == 1,
            'frontend positive inventory is metadata only')
    wall = p.get('seconds')
    require(type(wall) in (int, float) and math.isfinite(wall) and wall >= 0, 'missing frontend wall')
    require(measurement == {'label': row.target, 'mode': 'FRONTEND_ONLY', 'no_smt': True,
                           'proof': 'NOT_RUN', 'resource_acceptance': 'NOT_APPLICABLE'}, 'frontend/proof conflation')
    return {'frontend': 'PASS', 'mode': 'FRONTEND_ONLY', 'proof': 'NOT_RUN', 'no_smt': True,
            'resources': 'NOT_APPLICABLE', 'rlimit_run': None, 'rlimit_init': None,
            'wall_seconds': wall, 'wall_warning': wall > 20}


def result_stages(config, rows):
    frontend = config.mode == 'frontend-only'
    return {'resources': 'NOT_APPLICABLE' if frontend else ('PASS' if all(r['resources'] == 'PASS' for r in rows) else 'UNKNOWN'),
            'rlimit_run_sum': None if frontend or any(r.get('rlimit_run') is None for r in rows) else sum(r['rlimit_run'] for r in rows),
            'init_included_in_sum': False, 'mode': config.mode}


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
        row = {**asdict(selection), 'proof': 'UNKNOWN', 'resources': 'UNKNOWN',
               'collection': 'PASS', 'rlimit_run': None, 'rlimit_init': None,
               'total_ms': None, 'threads': None, 'wall_seconds': None, 'wall_warning': None}
        objects = {}
        invalid, unavailable = False, False
        try:
            # Bazel itself owns the testlogs link. Resolve that one boundary only.
            directory = (root / 'bazel-testlogs').resolve(strict=True)
            for component in selection.target[2:].replace(':', '/').split('/'):
                directory = directory / component
                require(not directory.is_symlink() and directory.is_dir(), 'unexpected testlog directory link/type')
            for name in ('test.log', 'test.xml', 'inventory.json', 'measurement.json', 'verifier.json', 'execution.json'):
                try:
                    data = regular_bytes(directory / name) if name in ('test.log', 'test.xml') else output_bytes(directory, name)
                    # Retain the actual failing XML/log/JSON before validation.
                    files.append(put(output, f'{index}-{name}', data))
                    require(data, 'empty selected child evidence')
                    if name == 'test.xml':
                        check_test_xml(data)
                    elif name.endswith('.json'):
                        objects[name] = loads(data)
                except OSError:
                    unavailable = True
                except (ValueError, ET.ParseError, zipfile.BadZipFile):
                    invalid = True
            if len(objects) == 4:
                try:
                    row.update(child_verdict(objects['verifier.json'], objects['inventory.json'], objects['measurement.json'], selection, config.mode, objects['execution.json'], config.raw_queries))
                except (ValueError, KeyError, TypeError):
                    invalid = True
            if config.raw_queries:
                stage_tree(directory / 'test.outputs' / 'raw-queries', output, f'{index}-query-', files)
        except OSError:
            unavailable = True
        except ValueError:
            invalid = True
        if invalid:
            row.update(proof='FAIL', collection='FAIL', reason='invalid_child_evidence')
        elif unavailable:
            row.update(collection='UNKNOWN', reason='child_evidence_unavailable')
        rows.append(row)
    run_values = [r['rlimit_run'] for r in rows]
    child = 'FAIL' if batch_exit != 0 or any(r['proof'] == 'FAIL' for r in rows) else (
        'PASS' if all(r.get('frontend' if config.mode == 'frontend-only' else 'proof') == 'PASS' for r in rows) else 'UNKNOWN')
    collection = 'FAIL' if any(r['collection'] == 'FAIL' for r in rows) else (
        'PASS' if all(r['collection'] == 'PASS' for r in rows) else 'UNKNOWN')
    return {'child': child, 'rows': rows, **result_stages(config, rows),
            'collection': collection, 'artifacts': files}


def same_tools(frontend, actual):
    require(actual.get('pins') == frontend['pins'] and all(
        {k: actual['tools'][name][k] for k in ('sha256', 'bytes')} ==
        {k: tool[k] for k in ('sha256', 'bytes')} for name, tool in frontend['tools'].items()),
        'frontend/measurement actual tool identity mismatch')


def validate_frontend_ticket(ticket, config, prepared):
    require(type(ticket) is dict and ticket.get('schema') == 'tl3.verus-measurement.frontend-prerequisite.v1' and
            ticket.get('no_smt') is True, 'missing frontend prerequisite; no measured execution allowed')
    for key in ('source_sha256', 'selection_sha256', 'runner_sha256', 'pins_sha256'):
        require(ticket.get(key) == prepared[key], 'frontend prerequisite source/runner/selection/pins mismatch')
    require(ticket.get('driver_sha256') == tooling_identity()['driver_sha256'], 'frontend prerequisite driver mismatch')
    frontend = Config.parse({**config.json(), 'mode': 'frontend-only', 'watchdog_seconds': 45,
                             'diagnostic_authorization': None, 'raw_queries': False})
    require(ticket.get('frontend_bazel_argv') == frontend.command() and
            type(ticket.get('parent_invocation_id')) is str and
            re.fullmatch(r'[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}', ticket['parent_invocation_id']), 'frontend prerequisite argv/parent mismatch')
    require(type(ticket.get('executions')) is list and len(ticket['executions']) == len(config.selections), 'missing frontend execution identities')
    for row, execution in zip(config.selections, ticket['executions']):
        validate_execution(execution, row, 'frontend-only')
    return ticket


def main():
    require(sys.version_info >= (3, 10), 'measurement driver requires Python 3.10 or later')
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config-json', required=True)
    parser.add_argument('--source', type=Path, default=Path.cwd())
    parser.add_argument('--output', type=Path, required=True, help='new owned directory')
    parser.add_argument('--dry-run', action='store_true')
    parser.add_argument('--frontend-prerequisite-json', default='null')
    args = parser.parse_args()
    config = Config.parse(loads(args.config_json))
    require(not args.output.resolve().is_relative_to(args.source.resolve()),
            'evidence output must be outside the clean source checkout')
    source_identity(args.source, config)
    output = owned_directory(args.output)
    prepared, original, changed = prepare(args.source, config, output)
    if args.dry_run:
        print(json.dumps(prepared))
        return
    ticket = None
    if config.mode != 'frontend-only':
        ticket = validate_frontend_ticket(loads(args.frontend_prerequisite_json), config, prepared)
        put(output, 'frontend-prerequisite.json', encoded(ticket))
    summary = {'preparation': 'PASS', 'restoration': 'UNKNOWN', 'collection': 'UNKNOWN',
               'child': 'UNKNOWN', 'resources': 'UNKNOWN',
               'command_artifact_namespace': 'verus-measurement',
               'reserved_name_collision': False}
    try:
        reserved = output.parent / 'bbrun-wrapper-result.v1.json'
        require(not reserved.exists() and not reserved.is_symlink(), 'reserved wrapper artifact collision before command')
        with disposable_runner(args.source, original, changed):
            run = subprocess.run(config.command(), cwd=args.source, check=False)
            summary['child'] = 'PASS' if run.returncode == 0 else 'FAIL'
            summary.update(collect(args.source, config, output, run.returncode))
            if ticket is not None:
                for index, frontend_execution in enumerate(ticket['executions']):
                    actual = loads(regular_bytes(output / f'{index}-execution.json'))
                    same_tools(frontend_execution, actual)
    except Exception:
        summary['collection'] = 'FAIL'
        raise
    finally:
        summary['reserved_name_collision'] = reserved.exists() or reserved.is_symlink()
        try:
            summary['restoration'] = 'PASS' if (regular_bytes(args.source / RUNNER) == original and
                digest(encoded(source_identity(args.source, config))) == prepared['source_sha256']) else 'FAIL'
        except (OSError, ValueError):
            summary['restoration'] = 'FAIL'
        # Parent/scanner/wrapper can only be observed after this command terminates.
        put(output, 'measurement-summary.json', encoded(summary))
        all_files = [{'name': p.name, 'sha256': digest(regular_bytes(p)), 'bytes': p.stat().st_size}
                     for p in sorted(output.iterdir())]
        put(output, 'artifact-manifest.json', encoded(all_files))
    require(all(summary.get(s) == 'PASS' for s in ('child', 'collection', 'restoration')) and
            summary['resources'] == ('NOT_APPLICABLE' if config.mode == 'frontend-only' else 'PASS'), 'measurement stages did not pass')


if __name__ == '__main__':
    main()
