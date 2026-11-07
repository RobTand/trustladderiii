"""Gate a single BB submission on a full exact-source dry-run receipt.

Run dry-runs on admitted PB or supported free remote compute, never a fleet host.
"""
from __future__ import annotations

import argparse
import base64
import zlib
import json
import os
from pathlib import Path
import py_compile
import re
import runpy
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
DRIVER = HERE / 'measure-remote.py'
D = runpy.run_path(str(DRIVER))
loads, require, encoded = D['loads'], D['require'], D['encoded']
exact_keys, digest = D['exact_keys'], D['digest']
BB = ['python3', '-I', '/home/rob/tmp/bazel-wave2/tools/bbrun.py']
HOLD = Path('/home/rob/tmp/tl3-build/rust-fv/BB-HOLD')
MARKER = 'BBRUN_WRAPPER_RESULT_V1 '
RESERVED = 'bbrun-wrapper-result.v1.json'


def submission(config, runner_log):
    # Ship the exact reviewed driver bytes; remote preparation rechecks the source.
    code = DRIVER.read_bytes()
    # Compressed single script, no full source/diff duplication in transport.
    payload = base64.b64encode(zlib.compress(code, 9)).decode()
    bootstrap = ('import base64,hashlib,os,sys,zlib;'
                 f'code=zlib.decompress(base64.b64decode({payload!r}));'
                 f'assert hashlib.sha256(code).hexdigest()=={digest(code)!r};'
                 'sys.argv += ["--output",os.path.join(os.environ["BUILDBUDDY_ARTIFACTS_DIRECTORY"],"verus-measurement")];'
                 f'exec(compile(code,"<reviewed-measure-remote>","exec"),dict(__name__="__main__",__driver_sha256__={digest(code)!r}))')
    return [*BB, 'step', '--repo', 'https://github.com/RobTand/trustladderiii.git',
            '--commit', config.head, '--branch', config.branch,
            '--timeout', str(config.coordinator_minutes), '--max-wait', '600',
            '--log-file', str(runner_log), '--', 'python3', '-I', '-c', bootstrap,
            '--config-json', json.dumps(config.json(), sort_keys=True)]



def identity(config, prepared, command):
    return {'schema': 'tl3.verus-measurement.dry-run', 'version': 1,
            'driver_sha256': digest(DRIVER.read_bytes()),
            'submit_sha256': digest(Path(__file__).read_bytes()),
            'source_sha256': prepared['source_sha256'],
            'config_sha256': prepared['config_sha256'],
            'selection_sha256': prepared['selection_sha256'],
            'instrumentation_sha256': prepared['instrumentation_sha256'],
            'runner_sha256': prepared['runner_sha256'], 'pins_sha256': prepared['pins_sha256'],
            'mode': config.mode,
            'exact_submission_argv': command}


def atomic_receipt(path, value):
    fd, temporary = tempfile.mkstemp(prefix='.receipt-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as stream:
            stream.write(encoded(value))
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def full_dry_run(source, config, output, receipt, command):
    # First action, before config/preparation/compilation can fail.
    receipt.unlink(missing_ok=True)
    require(not output.resolve().is_relative_to(source.resolve()), 'evidence output must be outside the clean source checkout')
    D['source_identity'](source, config)
    D['owned_directory'](output)
    for script in (DRIVER, Path(__file__)):
        py_compile.compile(str(script), cfile=str(output / (script.name + 'c')), doraise=True)
        (output / (script.name + 'c')).chmod(0o600)
    compile(command[command.index('-c') + 1], '<exact-remote-command>', 'exec')
    prepared, _, _ = D['prepare'](source, config, output)
    D['put'](output, 'submission-argv.json', encoded(command))
    result = {**identity(config, prepared, command), 'py_compile': 'PASS',
              'full_preparation': 'PASS', 'build_executed': False, 'buildbuddy_submitted': False}
    atomic_receipt(receipt, result)
    return result


def check_receipt(source, config, receipt, command):
    require(receipt.exists(), 'missing successful dry-run receipt; run --dry-run on a clean exact-head fixture')
    saved = loads(D['regular_bytes'](receipt))
    source_hash = digest(encoded(D['source_identity'](source, config)))
    patch = D['instrument'](D['regular_bytes'](source / D['RUNNER']).decode(), config)
    original = D['regular_bytes'](source / D['RUNNER']).decode()
    import difflib
    diff = ''.join(difflib.unified_diff(original.splitlines(True), patch.splitlines(True),
                                      fromfile='a/' + D['RUNNER'], tofile='b/' + D['RUNNER']))
    prepared = {'source_sha256': source_hash, 'config_sha256': digest(encoded(config.json())),
                'selection_sha256': digest(encoded([D['asdict'](s) for s in config.selections])),
                'instrumentation_sha256': digest(diff.encode()), 'runner_sha256': digest(patch.encode()), 'pins_sha256': digest(encoded(D['source_pins'](source)))}
    expected = {**identity(config, prepared, command), 'py_compile': 'PASS', 'full_preparation': 'PASS',
                'build_executed': False, 'buildbuddy_submitted': False}
    require(type(saved) is dict, 'malformed dry-run receipt; repeat --dry-run')
    differing = sorted(k for k in set(saved) | set(expected) if encoded(saved.get(k)) != encoded(expected.get(k)))
    require(not differing, 'stale/mismatched dry-run receipt fields: ' + ', '.join(differing) + '; repeat --dry-run')
    return expected


# Consumer checks only. Ops owns the authoritative schema, scanner and publication.
REASON_STAGES = ('command', 'artifact_handoff', 'scanner', 'cleanup')


def integer_or_null(value, wrapper=False):
    require(value is None or (type(value) is int and (not wrapper or 0 <= value <= 255)), 'invalid exit code')


def validate_wrapper(value, parent):
    exact_keys(value, ('schema', 'version', 'parent_invocation_id', 'command', 'artifact_handoff', 'scanner', 'cleanup', 'wrapper'))
    require(value['schema'] == 'tl3.bbrun.wrapper-result' and type(value['version']) is int and value['version'] == 1, 'unsupported wrapper interface')
    require(re.fullmatch(r'[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}', parent) is not None, 'invalid own parent ID')
    require(value['parent_invocation_id'] is None or value['parent_invocation_id'] == parent, 'own-parent attribution mismatch')
    fields = {'command': ('status', 'exit_code', 'reason'), 'artifact_handoff': ('status', 'reason'),
              'scanner': ('status', 'first_exit_code', 'exit_code', 'reason'),
              'cleanup': ('status', 'targets_present', 'reason'), 'wrapper': ('original_exit_code', 'exit_code')}
    for stage, keys in fields.items():
        exact_keys(value[stage], keys)
    for stage in REASON_STAGES:
        reason = value[stage]['reason']
        require(type(reason) is str and re.fullmatch(r'[a-z][a-z0-9_]{0,95}', reason),
                'invalid wrapper-owned sanitized reason code')
    c, h, s, clean, w = (value[k] for k in fields)
    for code in (c['exit_code'], s['first_exit_code'], s['exit_code']):
        integer_or_null(code)
    for code in w.values():
        integer_or_null(code, wrapper=True)
    require(c['status'] == ('unknown' if c['exit_code'] is None else 'pass' if c['exit_code'] == 0 else 'fail'), 'contradictory command status')
    if c['status'] == 'pass':
        require(c['reason'] == 'command_succeeded', 'contradictory command pass reason')
    require(h['status'] in ('pass', 'fail', 'unknown'), 'invalid handoff status')
    if h['status'] == 'pass':
        require(h['reason'] == 'handoff_succeeded', 'contradictory handoff pass reason')
    require(s['status'] in ('pass', 'fail', 'error', 'unknown'), 'invalid scanner status')
    if s['status'] == 'pass':
        require(s['exit_code'] == 0 and s['first_exit_code'] is not None and s['reason'] == 'scan_clean', 'contradictory scanner pass')
    elif s['status'] == 'fail':
        require(s['exit_code'] not in (None, 0) and s['first_exit_code'] is not None, 'contradictory scanner fail')
    require(clean['status'] in ('pass', 'not_needed', 'fail', 'unknown') and
            (clean['targets_present'] is None or type(clean['targets_present']) is bool), 'invalid cleanup')
    if clean['status'] == 'pass':
        require(clean['targets_present'] is True and clean['reason'] == 'cleanup_complete', 'contradictory cleanup pass')
    elif clean['status'] == 'not_needed':
        require(clean['targets_present'] is False and clean['reason'] == 'no_cleanup_targets', 'contradictory cleanup not_needed')
    upstream = c['status'] == h['status'] == s['status'] == 'pass'
    if w['original_exit_code'] == 0:
        require(upstream, 'wrapper original zero contradicts stages')
    if w['exit_code'] == 0:
        require(w['original_exit_code'] == 0 and clean['status'] in ('pass', 'not_needed'), 'wrapper zero contradicts original/cleanup')
    if upstream and w['original_exit_code'] is not None:
        require(w['original_exit_code'] == 0, 'wrapper original contradicts upstream success')
    if clean['status'] in ('pass', 'not_needed') and all(code is not None for code in w.values()):
        require(w['original_exit_code'] == w['exit_code'], 'successful cleanup changed wrapper exit')
    return value


def wrapper_evidence(*, parent, terminal, trusted_artifact=None, trusted_runner_line=None,
                     command_reserved_collision=False):
    """Call only at the trusted collector boundary with own-parent CAS bytes.

    A path/name or command-produced file is never trusted_artifact. The collector
    must exclude command-origin markers before supplying a wrapper-owned line.
    The driver deliberately has no API that promotes arbitrary log text to trust.
    """
    require(terminal in ('PASS', 'FAIL'), 'own parent is not terminal; wrapper remains UNKNOWN')
    require(not command_reserved_collision, 'command-created reserved-name collision')
    values = []
    if trusted_artifact is not None:
        values.append(validate_wrapper(loads(trusted_artifact), parent))
    if trusted_runner_line is not None:
        require(type(trusted_runner_line) is str and trusted_runner_line.count(MARKER) == 1 and
                trusted_runner_line.startswith(MARKER) and '\n' not in trusted_runner_line,
                'forged/repeated/ambiguous wrapper marker')
        values.append(validate_wrapper(loads(trusted_runner_line[len(MARKER):]), parent))
    require(not values or all(v == values[0] for v in values), 'wrapper artifact/line disagree')
    return values[0] if values else None


def qualify(driver, wrapper, parent):
    stages = ('preparation', 'child', 'collection', 'restoration')
    if driver.get('mode') != 'frontend-only':
        stages += ('resources',)
    elif driver.get('resources') != 'NOT_APPLICABLE' or not all(r.get('no_smt') is True for r in driver.get('rows', [])) or not driver.get('rows'):
        return 'UNKNOWN'
    if parent == 'FAIL' or any(driver.get(k) == 'FAIL' for k in stages):
        return 'FAIL'
    if wrapper is None or parent != 'PASS':
        return 'UNKNOWN'
    if any(wrapper[k]['status'] in ('fail', 'error') for k in ('command', 'artifact_handoff', 'scanner', 'cleanup')) or any(
            code is not None and code != 0 for code in (wrapper['command']['exit_code'], *wrapper['wrapper'].values())):
        return 'FAIL'
    good = (all(driver.get(k) == 'PASS' for k in stages) and parent == 'PASS' and
            all(wrapper[k]['status'] == 'pass' for k in ('command', 'artifact_handoff', 'scanner')) and
            wrapper['cleanup']['status'] in ('pass', 'not_needed') and
            wrapper['command']['exit_code'] == wrapper['wrapper']['original_exit_code'] == wrapper['wrapper']['exit_code'] == 0)
    return 'PASS' if good else 'UNKNOWN'


def cas_artifact(parent, metadata, output, local_name):
    """Fetch only an exact own-parent artifact and verify its CAS hash/length."""
    uri = metadata.get('uri', '')
    match = re.search(r'/blobs/([0-9a-f]{64})/([0-9]+)$', uri)
    require(match is not None, 'artifact lacks verifiable CAS hash/length')
    path = output / local_name
    # bbrun's downloader writes bytes only; reserve our private regular file first.
    D['put'](output, local_name, b'')
    fetched = subprocess.run([*BB, 'artifacts', parent, '--fetch', metadata['name'], '--out', str(path)],
                             stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    if fetched.returncode != 0:
        raise OSError('own-parent CAS artifact unavailable')
    data = D['regular_bytes'](path)
    require(digest(data) == match[1] and len(data) == int(match[2]), 'CAS hash/length mismatch')
    return data, {'parent_invocation_id': parent, 'name': metadata['name'], 'uri': uri,
                  'sha256': match[1], 'bytes': len(data)}


def collect_terminal(parent, terminal, output, runner_log, config, expected, *, wrapper_step_stdout=None):
    require(terminal in ('PASS', 'FAIL'), 'own parent must be independently terminal')
    listed = subprocess.run([*BB, 'artifacts', parent], stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, text=True)
    if listed.returncode != 0:
        raise OSError('own-parent artifact enumeration unavailable')
    metadata = [loads(line) for line in listed.stdout.splitlines() if line.startswith('{')]
    D['put'](output, 'own-parent-artifacts.json', encoded(metadata))
    def named(name):
        found = [m for m in metadata if m.get('name') == name]
        require(len(found) <= 1, 'ambiguous own-parent artifact: ' + name)
        return found[0] if found else None
    audits = []
    wrapper_meta = named(RESERVED)
    summary_meta = named('verus-measurement/measurement-summary.json')
    manifest_meta = named('verus-measurement/artifact-manifest.json')
    # A terminal parent without the driver's namespace/collision evidence cannot
    # establish that a reserved artifact or log marker came from the wrapper.
    if summary_meta is None or manifest_meta is None:
        return {'parent': terminal, 'wrapper': 'UNKNOWN', 'overall': 'FAIL' if terminal == 'FAIL' else 'UNKNOWN',
                'reason': 'missing_driver_namespace_evidence', 'cas': audits}
    summary_bytes, audit = cas_artifact(parent, summary_meta, output, 'remote-measurement-summary.json')
    audits.append(audit)
    summary = loads(summary_bytes)
    require(summary.get('command_artifact_namespace') == 'verus-measurement' and
            summary.get('reserved_name_collision') is False, 'command-created reserved-name collision or unknown namespace')
    manifest_bytes, audit = cas_artifact(parent, manifest_meta, output, 'remote-artifact-manifest.json')
    audits.append(audit)
    manifest = loads(manifest_bytes)
    require(type(manifest) is list and len({m['name'] for m in manifest}) == len(manifest), 'duplicate/malformed driver manifest')
    # Verify and retain every curated raw file. No arbitrary parent bulk download.
    curated = {}
    for index, entry in enumerate(manifest):
        exact_keys(entry, ('name', 'sha256', 'bytes'))
        require(Path(entry['name']).name == entry['name'] and type(entry['bytes']) is int and entry['bytes'] >= 0,
                'unsafe driver manifest entry')
        require(entry['name'] != RESERVED, 'command-created reserved-name collision')
        item = named('verus-measurement/' + entry['name'])
        require(item is not None, 'missing curated own-parent artifact: ' + entry['name'])
        data, audit = cas_artifact(parent, item, output, f'curated-{index}')
        require(digest(data) == entry['sha256'] and len(data) == entry['bytes'], 'driver manifest mismatch')
        audits.append(audit)
        curated[entry['name']] = data
    for name, identity_key in (('source.json', 'source_sha256'), ('config.json', 'config_sha256'),
                               ('selections.json', 'selection_sha256'), ('runner.patch', 'instrumentation_sha256'),
                               ('runner-preview.rs', 'runner_sha256'), ('pins.json', 'pins_sha256')):
        require(name in curated and digest(curated[name]) == expected[identity_key], 'remote preparation identity mismatch: ' + name)
    require(loads(curated['config.json']) == config.json() and loads(curated['command.json']) == config.command(),
            'remote config/command mismatch')
    require(loads(curated['tooling.json']) == D['tooling_identity'](), 'executed driver tooling provenance mismatch')
    require(curated.get('measurement-summary.json') == summary_bytes, 'summary/manifest disagreement')
    rows = []
    for index, selection in enumerate(config.selections):
        for name in ('test.log', 'test.xml', 'inventory.json', 'measurement.json', 'verifier.json', 'execution.json'):
            require(f'{index}-{name}' in curated and curated[f'{index}-{name}'], 'missing raw selected-child receipt')
        D['check_test_xml'](curated[f'{index}-test.xml'])
        require(loads(curated[f'{index}-execution.json']).get('pins') == loads(curated['pins.json']), 'actual/source toolchain pins differ')
        rows.append(D['child_verdict'](loads(curated[f'{index}-verifier.json']),
                                      loads(curated[f'{index}-inventory.json']),
                                      loads(curated[f'{index}-measurement.json']), selection, config.mode, loads(curated[f'{index}-execution.json'])))
    require(len(summary.get('rows', [])) == len(config.selections), 'missing/duplicate measurement rows')
    for row, selection in zip(summary['rows'], config.selections):
        require(all(row.get(k) == v for k, v in D['asdict'](selection).items()), 'summary selection/count/bijection mismatch')
    # Only verified raw receipts determine the returned metrics. Summary values
    # cannot supply a measurement that is absent from the child evidence.
    require(summary.get('restoration') == 'PASS' and summary.get('child') == 'PASS' and
            summary.get('collection') == 'PASS' and summary.get('preparation') == 'PASS', 'remote stages did not pass')
    summary['rows'] = [{**D['asdict'](selection), **row} for selection, row in zip(config.selections, rows)]
    summary.update(D['result_stages'](config, rows))
    for stage in ('parent', 'scanner', 'cleanup', 'overall', 'artifacts'):
        summary.pop(stage, None)
    wrapper_bytes = None
    if wrapper_meta is not None:
        wrapper_bytes, audit = cas_artifact(parent, wrapper_meta, output, RESERVED)
        audits.append(audit)
    lines = [line for line in runner_log.splitlines() if MARKER in line]
    require(len(lines) <= 1 and all(line.startswith(MARKER) for line in lines), 'forged/repeated/ambiguous wrapper markers')
    # The full parent log includes BuildBuddy footers and command stdout. It is
    # not a proven wrapper-step body and cannot independently authorize fallback.
    trusted_line = None
    if wrapper_step_stdout is not None:
        body = [line for line in wrapper_step_stdout.splitlines() if line.strip()]
        markers = [line for line in body if MARKER in line]
        require(len(markers) == 1 and markers[0].startswith(MARKER) and body[-1] == markers[0],
                'wrapper body lacks an unambiguous final result line')
        trusted_line = markers[0]
    wrapper = wrapper_evidence(parent=parent, terminal=terminal, trusted_artifact=wrapper_bytes,
                               trusted_runner_line=trusted_line)
    if wrapper is not None and lines:
        require(validate_wrapper(loads(lines[0][len(MARKER):]), parent) == wrapper,
                'wrapper artifact/body and parent-log line disagree')
    return {'parent': terminal, 'wrapper': wrapper if wrapper is not None else 'UNKNOWN',
            'driver': summary, 'overall': qualify(summary, wrapper, terminal), 'cas': audits}


def wait_own(command, output, config, expected):
    """One submission, then waits on the same parent. Timeout never resubmits."""
    require(not HOLD.exists(), 'paused for BB hold')
    result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    transcript = result.stdout
    ids = set(re.findall(r'(?m)^https://app\.buildbuddy\.io/invocation/([0-9a-f-]{36})$', transcript))
    require(len(ids) == 1, 'submission lacks unique own invocation; UNKNOWN, do not resubmit')
    parent = ids.pop()
    D['put'](output, 'submission-client.txt', transcript.encode())
    waits = 0
    wait_limit = max(1, (config.coordinator_minutes * 60 + 599) // 600)
    wait_reason = None
    while result.returncode == 2:
        pending = re.search(r'(?m)^still running: ' + re.escape(parent) + r'; resume with: bbrun\.py wait ' + re.escape(parent) + r'\s*$', result.stdout)
        if pending is None:
            wait_reason = 'client_wait_not_resumable'
            break
        if waits >= wait_limit:
            wait_reason = 'same_invocation_wait_limit'
            break
        waits += 1
        result = subprocess.run([*BB, 'wait', parent, '--max-wait', '600'],
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        transcript += result.stdout
    D['put'](output, 'terminal-client.txt', transcript.encode())
    # Inspect using the approved scrubbed result reader. Installed wrappers
    # without v1 remain UNKNOWN even when the command and children passed.
    observation = {'parent_invocation_id': parent, 'client_exit': result.returncode,
                   'parent': 'UNKNOWN',
                   'wrapper': 'UNKNOWN', 'overall': 'UNKNOWN',
                   'reason': 'trusted_wrapper_cas_collection_required'}
    malformed = False
    for line in transcript.splitlines():
        if line.startswith('verdict: '):
            try:
                require(len(line) <= 16384, 'oversized terminal verdict')
                verdict = loads(line[len('verdict: '):])
                require(type(verdict) is dict, 'malformed terminal verdict')
                if verdict.get('invocationId') == parent and verdict.get('invocationStatus') == 'COMPLETE_INVOCATION_STATUS':
                    require(type(verdict.get('success')) is bool, 'missing terminal success verdict')
                    if observation['parent'] != 'FAIL':
                        observation['parent'] = 'PASS' if verdict['success'] else 'FAIL'
            except (ValueError, TypeError):
                malformed = True
    if observation['parent'] == 'FAIL':
        observation['overall'] = 'FAIL'
    try:
        inspected = subprocess.run(['python3', '-I', '/home/rob/tmp/tl3-build/bb_logs.py', parent, '--runner', '200'],
                                   stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        D['put'](output, 'terminal-runner.txt', inspected.stdout.encode())
    except OSError:
        inspected = None
    if wait_reason or malformed:
        observation.update(collection='UNKNOWN', reason=wait_reason or 'invalid_terminal_verdict')
    elif observation['parent'] != 'UNKNOWN' and inspected is not None and inspected.returncode == 0:
        try:
            observation.update(collect_terminal(parent, observation['parent'], output, inspected.stdout, config, expected))
        except OSError:
            observation.update(collection='UNKNOWN', overall='FAIL' if observation['parent'] == 'FAIL' else 'UNKNOWN',
                               reason='terminal_evidence_unavailable')
        except (ValueError, KeyError, TypeError, D['ET'].ParseError):
            observation.update(collection='FAIL', overall='FAIL', reason='terminal_evidence_invalid')
    else:
        observation.update(collection='UNKNOWN', reason='terminal_evidence_unavailable')
    observation['preparation_identity'] = expected
    observation['config'] = config.json()
    D['put'](output, 'terminal-attribution.json', encoded(observation))
    return {'PASS': 0, 'FAIL': 1, 'UNKNOWN': 3}[observation['overall']]


def check_frontend(source, config, path, output):
    require(path is not None, 'missing frontend prerequisite; normal/diagnostic submission refused')
    prior = loads(D['regular_bytes'](path))
    frontend = D['Config'].parse({**config.json(), 'mode': 'frontend-only',
        'watchdog_seconds': 45, 'diagnostic_authorization': None, 'raw_queries': False})
    require(prior.get('overall') == 'PASS' and prior.get('config') == frontend.json(), 'failed/stale frontend mode/source/selection prerequisite')
    expected = prior.get('preparation_identity', {})
    require(expected.get('driver_sha256') == digest(DRIVER.read_bytes()) and
            expected.get('submit_sha256') == digest(Path(__file__).read_bytes()) and
            expected.get('source_sha256') == digest(encoded(D['source_identity'](source, frontend))) and
            expected.get('runner_sha256') == digest(D['regular_bytes'](source / D['RUNNER'])) and
            expected.get('mode') == 'frontend-only', 'stale frontend tooling/source/runner prerequisite')
    argv = expected.get('exact_submission_argv', [])
    require('--log-file' in argv and argv == submission(frontend, Path(argv[argv.index('--log-file') + 1])), 'stale frontend argv prerequisite')
    parent = prior.get('parent_invocation_id')
    require(type(parent) is str and re.fullmatch(r'[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}', parent), 'missing own frontend parent')
    # Re-read the own terminal verdict and all scanned CAS bytes. The local file
    # is a locator, never authority for PASS or no-SMT evidence.
    terminal = subprocess.run([*BB, 'wait', parent, '--max-wait', '1'], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    D['put'](output, 'frontend-parent-terminal.txt', terminal.stdout.encode())
    verdicts = [loads(line[len('verdict: '):]) for line in terminal.stdout.splitlines() if line.startswith('verdict: ')]
    require(terminal.returncode == 0 and len(verdicts) == 1 and verdicts[0].get('invocationId') == parent and
            verdicts[0].get('invocationStatus') == 'COMPLETE_INVOCATION_STATUS' and verdicts[0].get('success') is True,
            'frontend own parent is not terminal PASS')
    inspected = subprocess.run(['python3', '-I', '/home/rob/tmp/tl3-build/bb_logs.py', parent, '--runner', '200'], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    require(inspected.returncode == 0, 'frontend own terminal logs unavailable')
    audited = collect_terminal(parent, 'PASS', D['owned_directory'](output / 'frontend-audit'), inspected.stdout, frontend, expected)
    D['put'](output, 'frontend-prerequisite.json', encoded(audited))
    require(audited['overall'] == 'PASS', 'frontend independently scanned evidence did not pass')
    return audited


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True, help='new owned evidence directory')
    parser.add_argument('--receipt', type=Path, required=True)
    parser.add_argument('--runner-log', type=Path, required=True)
    parser.add_argument('--dry-run', action='store_true')
    parser.add_argument('--frontend-receipt', type=Path, help='own frontend terminal-attribution.json; reaudited before measured submission')
    args = parser.parse_args()
    # Invalidate even when config parsing fails, not merely inside preparation.
    if args.dry_run:
        args.receipt.unlink(missing_ok=True)
    config = D['Config'].parse(loads(args.config.read_bytes()))
    command = submission(config, args.runner_log)
    if args.dry_run:
        result = full_dry_run(args.source, config, args.output, args.receipt, command)
        print(json.dumps({k: v for k, v in result.items() if k != 'exact_submission_argv'}, indent=2))
        return
    checked = check_receipt(args.source, config, args.receipt, command)
    require(not args.output.resolve().is_relative_to(args.source.resolve()), 'output inside source')
    D['owned_directory'](args.output)
    if config.mode != 'frontend-only':
        check_frontend(args.source, config, args.frontend_receipt, args.output)
    D['put'](args.output, 'submission-argv.json', encoded(command))
    if HOLD.exists():
        raise SystemExit('paused for BB hold')
    raise SystemExit(wait_own(command, args.output, config, checked))


if __name__ == '__main__':
    main()
