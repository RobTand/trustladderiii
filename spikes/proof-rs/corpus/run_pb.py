"""One admitted build-and-replay cycle. All computation happens on the PB worker."""
import base64
import io
import json
from pathlib import Path
import platform
import subprocess
import sys
import tarfile
import time

root = Path(__file__).resolve().parents[1]
out = root / 'measurement-output'
out.mkdir(exist_ok=True)

def run(argv, name, *, stdin=None):
    started = time.perf_counter()
    with (out / (name + '.log')).open('wb') as log:
        result = subprocess.run(argv, cwd=root, stdin=stdin, stdout=log, stderr=subprocess.STDOUT)
    info = {'command': argv, 'returncode': result.returncode, 'seconds': time.perf_counter() - started}
    print('STEP ' + json.dumps(info), flush=True)
    if result.returncode:
        print((out / (name + '.log')).read_text()[-24000:], flush=True)
    return info

summary = {'platform': platform.platform(), 'machine': platform.machine(), 'steps': []}
summary['steps'].append(run(['cargo', 'fmt'], 'fmt'))
summary['steps'].append(run(['cargo', 'build', '--release', '-j', '4'], 'build'))
if summary['steps'][-1]['returncode'] == 0:
    summary['steps'].append(run(['cargo', 'clippy', '--all-targets', '--locked', '-j', '4', '--', '-D', 'warnings'], 'clippy'))
    # Cargo metadata is used for dependency count and SPDX license inventory.
    summary['steps'].append(run(['cargo', 'metadata', '--format-version', '1', '--locked'], 'metadata'))
    for mode in ('current', 'reduced'):
        with (root / 'corpus/data/calls.jsonl').open('rb') as stream:
            command = [str(root / 'target/release/proof-rs-spike')]
            if mode == 'reduced':
                command.append('--reduced')
            summary['steps'].append(run(command, mode, stdin=stream))
    with (root / 'corpus/data/sources.jsonl').open('rb') as stream:
        summary['steps'].append(run([str(root / 'target/release/proof-rs-spike'), '--sources'], 'sources', stdin=stream))
    summary['steps'].append(run([sys.executable, 'corpus/compare.py', 'corpus/data', str(out)], 'comparison'))
(out / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
if (root / 'Cargo.lock').exists():
    (out / 'Cargo.lock').write_bytes((root / 'Cargo.lock').read_bytes())
(out / 'main.rs').write_bytes((root / 'src/main.rs').read_bytes())
archive = io.BytesIO()
with tarfile.open(fileobj=archive, mode='w:gz') as tar:
    for path in sorted(out.iterdir()):
        tar.add(path, arcname=path.name)
print('SPIKE902_ARCHIVE ' + base64.b64encode(archive.getvalue()).decode(), flush=True)
raise SystemExit(summary['steps'][1]['returncode'])
