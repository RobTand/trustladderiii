#!/usr/bin/env python3
"""Served-input regression through the generated Checking action on a fixture Flux launch context.

Flux is the only executor (PB-REMOVAL). The Rust run backend normally supplies the
harness's launch context; here a FIXTURE context stands in for it: a private job
scratch leaf, an output directory with the progress journal, and (only when the
caller provides none) a private job-owned XDG_RUNTIME_DIR. Nothing else is replaced:
the harness admission and rootless checks, action.py, the real runner subprocess
(the container command is the runner, as in harness/tests/test_red_message.py) and
judgment all run for real. This creates temporary test inputs, never a canary packet
or native request, and it exercises no Rust admission or binding.
"""
import hashlib
import json
import os
import re
from pathlib import Path
import runpy
import shutil
import subprocess
import sys
import time

SOURCE = Path(__file__).resolve().parents[1]
BASE = '5dd6443393f8d1a6be6fd4ad7aaebc8e1af7f436'
NODE = 'test_value.py::test_value'
CONTEXT_ENV = 'FX_FIXTURE_FLUX_CONTEXT'


def fixture_job(root, name):
    """Create one fixture Flux job: the launch context the Rust backend would hand the harness."""
    job = root / name
    job.mkdir(mode=0o700)
    output = job / 'output'
    output.mkdir(mode=0o700)
    runtime_dir = job / 'xdg-runtime'
    runtime_dir.mkdir(mode=0o700)
    context = {'run': {'backend': 'flux', 'uri': 'local:///fixture/served', 'instance': 'fixture',
                       'job': 'f' + hashlib.sha256(str(job).encode()).hexdigest()[:12]},
               'scratch': {'leaf': str(job / 'scratch')},
               'progress': {'journal': str(output / 'progress.jsonl'), 'no_progress_seconds': 120},
               'output_dir': str(output)}
    path = job / 'context.json'
    path.write_text(json.dumps({'context': context, 'xdg_runtime_dir': str(runtime_dir)}))
    return path


def install_job(runtime, path):
    """Select Flux and give the harness this fixture job's launch context; nothing else changes."""
    job = json.loads(Path(path).read_text())
    os.environ['TL3_EXECUTION_BACKEND'] = 'flux'
    os.environ.setdefault('XDG_RUNTIME_DIR', job['xdg_runtime_dir'])
    runtime.launch_context = lambda: job['context']
    return job['context']


def action(staged, evidence):
    sys.path.insert(0, str(staged / 'trusted'))
    from tl3_harness import execution, runtime
    context = install_job(runtime, os.environ[CONTEXT_ENV])
    execution.require_admission()
    runner = Path(execution.__file__).with_name('runner.py')
    execution.container_command = lambda tree, container, tests: [
        sys.executable, '-I', '-B', str(runner), '-', str(tree)]
    actual = execution.run_tree
    runs = []

    def capture(tree, container, tests, report, *args, **kwargs):
        started = time.monotonic()
        output = Path(context['output_dir'])
        before = set(output.glob(f'{report.stem}-*.jsonl'))
        outcomes = actual(tree, container, tests, report, *args, **kwargs)
        # Under Flux the harness journals each report into the job's output_dir.
        journal, = set(output.glob(f'{report.stem}-*.jsonl')) - before
        records = journal.read_bytes()
        target = evidence / f'{len(runs)}-{report.name}'
        target.write_bytes(records)
        runs.append({'report': target.name, 'seconds': time.monotonic() - started,
                     'outcomes': outcomes, 'records_sha256': hashlib.sha256(records).hexdigest()})
        (evidence / 'runs.json').write_text(json.dumps(runs, indent=2))
        print(json.dumps({'raw_runner': target.name, 'records': [json.loads(line) for line in records.splitlines()]}), flush=True)
        return outcomes

    execution.run_tree = capture
    runpy.run_path(str(staged / 'action.py'), run_name='__main__')


def verify(root):
    from unittest.mock import patch
    sys.path.insert(0, str(SOURCE / 'harness/src'))
    from tl3_harness import runtime
    from tl3_harness.execution import require_admission
    from tl3_harness.trees import git
    install_job(runtime, fixture_job(root, 'verify-flux-job'))
    require_admission()
    evidence = Path(os.environ['FX_FIXTURE_EVIDENCE'])
    evidence.mkdir(parents=True, exist_ok=False)
    deliveries = {kind: json.loads((root / f'{kind}.json').read_bytes())
                  for kind in ('success', 'cancel')}
    assert deliveries['success']['head_sha'] != deliveries['cancel']['head_sha']
    # Original exact-5dd fBEL input, with the original bare assertion and no proof.
    original = root / 'original'
    original.mkdir()
    git(original, 'init', '-q', '--template=')
    old_source = git(SOURCE, 'show', f'{BASE}:crates/controller/examples/fx-stage-packet.rs').decode()
    assert '"from value import value\\ndef test_value(): assert value() == 2\\n"' in old_source
    (original / 'pyproject.toml').write_text(
        "[tool.tl3.harness]\ncommand = ['python', '-m', 'pytest']\ntest-files = ['test_*.py']\n")
    (original / 'test_value.py').write_text('from value import value\ndef test_value(): assert value() == 2\n')
    revisions = []
    for value in (1, 2):
        (original / 'value.py').write_text(f'def value(): return {value}\n')
        git(original, 'add', '.')
        git(original, '-c', 'user.name=fixture', '-c', 'user.email=fixture@localhost',
            'commit', '-qm', 'original fixture')
        revisions.append(git(original, 'rev-parse', 'HEAD').decode().strip())
    baseline = json.loads(json.dumps(deliveries['success']))
    baseline['checks']['repository'] = str(original)
    baseline['checks']['base_sha'] = baseline['checks']['reviewed_sha'] = revisions[0]
    baseline['head_sha'] = revisions[1]
    del baseline['checks']['plan'][0]['proof']
    deliveries = {'red-original': baseline, 'green-success': deliveries['success'],
                  'green-cancel': deliveries['cancel']}
    summary = {}
    for name, delivery in deliveries.items():
        retained = evidence / name
        retained.mkdir()
        (retained / 'delivery.json').write_text(json.dumps(delivery, indent=2))
        checks = delivery['checks']
        repo = Path(checks['repository'])
        staged = root / name
        staged.mkdir()
        shutil.copytree(SOURCE / 'harness/src/tl3_harness', staged / 'trusted/tl3_harness')
        shutil.copyfile(SOURCE / 'crates/controller/src/checks/action.py', staged / 'action.py')
        git(repo, 'bundle', 'create', str(staged / 'served.bundle'), '--all')
        request = {'effect_key': delivery['key'], 'base': checks['base_sha'], 'head': delivery['head_sha'],
                   'reviewed': checks['reviewed_sha'], 'plan': checks['plan'], 'image': 'sha256:' + '1' * 64,
                   'footprint': delivery['paths'], 'protected_paths': [], 'generated_paths': [],
                   'selected_failures': [], 'always_tests': [], 'selected_tests': None}
        (staged / 'request.json').write_text(json.dumps(request))
        (retained / 'request.json').write_text(json.dumps(request, indent=2))
        for side, revision in (('base', request['base']), ('head', request['head'])):
            for file in ('value.py', 'test_value.py', 'pyproject.toml'):
                (retained / f'{side}-{file}').write_bytes(git(repo, 'show', f'{revision}:{file}'))
        started = time.monotonic()
        child = subprocess.run([sys.executable, __file__, '--action', str(staged), str(retained)],
                               capture_output=True, timeout=55,
                               env={**os.environ, CONTEXT_ENV: str(fixture_job(root, f'{name}-flux-job'))})
        elapsed = time.monotonic() - started
        (retained / 'stdout.log').write_bytes(child.stdout)
        (retained / 'stderr.log').write_bytes(child.stderr)
        (retained / 'process.json').write_text(json.dumps({'exit_code': child.returncode, 'seconds': elapsed}))
        assert child.returncode == 0, child.stderr.decode()
        report, = [json.loads(line.removeprefix('TL3_CHECKS ')) for line in child.stdout.decode().splitlines()
                   if line.startswith('TL3_CHECKS ')]
        result, = report['results']
        test, = result['tests']
        assert test['base'] == 'assertion' and test['head'] == 'passed', result
        runs = json.loads((retained / 'runs.json').read_bytes())
        records = [json.loads(line) for run in runs
                   for line in (retained / run['report']).read_text().splitlines()]
        assert any(record.get('event') == 'finish' and record.get('exit_code') == 1 for record in records)
        assert any(record.get('event') == 'finish' and record.get('exit_code') == 0 for record in records)
        if name == 'red-original':
            assert result['verdict'] == 'fail', result
            assert test['message'] == 'Assertion RED needs a declared assertion reason in proof.red_message.', result
        else:
            assert result['verdict'] == 'pass' and test['passed'], result
            assert result['proof']['red_message_matches'] == {NODE: 'message'}, result
            assert result['proof']['red_classes'] == {NODE: 'assertion'}, result
            assert any(record.get('reason') == 'sandbox value must be 2' for record in records)
        if name == 'green-cancel':
            assert 24 <= elapsed < 55, elapsed
            assert all(12 <= run['seconds'] < 25 for run in runs), runs
        summary[name] = {'verdict': result['verdict'], 'seconds': elapsed,
                         'runner_processes': len(runs), 'process_exit': child.returncode}
        print(json.dumps({name: summary[name], 'served_report': report}), flush=True)
    # Driver caller coverage checks argv and explicit metadata without preparing a packet.
    driver = runpy.run_path(str(SOURCE / 'tools/fx2_stage_packet.py'))
    for kind in ('success', 'cancel'):
        calls = []
        fake = root / f'driver-{kind}'
        fake.mkdir()
        (fake / 'packet.json').write_text(json.dumps({
            'status': 'HELD_NOT_RELEASED_NOT_EXECUTED', 'runtime_qualified': False,
            'generation': 'fixture-generation', 'fixture': {'kind': kind, 'wait_seconds': 12 if kind == 'cancel' else 0},
            'source_snapshot': BASE}))
        binary = root / 'binary-identity-only'
        binary.write_bytes(b'not executable')
        argv = ['fx2_stage_packet.py', str(binary), hashlib.sha256(binary.read_bytes()).hexdigest(),
                str(fake), 'unused-manifest', 'unused-context', 'fixture-generation']
        if kind == 'cancel':
            argv += ['--fixture-kind', kind]
        with patch.object(sys, 'argv', argv), patch.object(subprocess, 'run', lambda args, **kw: calls.append(args)):
            driver['main']()
        assert calls[0][-2:] == ['--fixture-kind', kind], calls
    summary['driver_cases'] = 2
    summary['source_base'] = BASE
    summary['source_snapshot'] = git(SOURCE, 'rev-parse', 'HEAD').decode().strip()
    changed = set(git(SOURCE, 'diff', '--name-only', BASE).decode().splitlines())
    metadata = {name for name in changed if re.fullmatch(r'\.pbrun-closure\.[0-9a-f]{16}\.json', name)}
    summary['changed_paths'] = sorted(changed - metadata)
    summary['snapshot_metadata'] = sorted(metadata)
    summary['original_fixture_source_sha256'] = hashlib.sha256(old_source.encode()).hexdigest()
    summary['production_sha256'] = {str(path.relative_to(SOURCE)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in (SOURCE / 'harness/src/tl3_harness/model.py', SOURCE / 'crates/harness-core/src/judge.rs',
                     SOURCE / 'crates/controller/src/checks/action.py')}
    (evidence / 'summary.json').write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary), flush=True)
    print('FX_FIXTURE_EVIDENCE ' + json.dumps({str(path.relative_to(evidence)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(evidence.rglob('*')) if path.is_file()}), flush=True)


if __name__ == '__main__':
    if len(sys.argv) == 4 and sys.argv[1] == '--action':
        action(Path(sys.argv[2]), Path(sys.argv[3]))
    else:
        verify(Path(sys.argv[1]))
