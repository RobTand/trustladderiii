"""EP's BuildBuddy-only generation and final qualification driver."""
import gzip
import json
import os
from pathlib import Path
import runpy
import subprocess
import sys


def run(*args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def prepare():
    # Reuse the pinned in-tree bootstrap, without its unrelated P5 generators.
    runpy.run_path('workers/tests/p5/remote.py')['bootstrap']()
    run('bash', 'tools/bazel/generate.sh')
    run('bazel', 'run', '--config=remote', '--jobs=16', '//tools/bazel:worker_python', '--',
        str(Path(__file__).resolve()), 'identities')
    run('python3', 'crates/controller/src/prompt_render/oracle.py')
    run('bazel', 'build', '--config=remote', '--jobs=16', '--remote_download_toplevel', '//crates/controller:tl3')
    binary = str(Path('bazel-bin/crates/controller/tl3').resolve())
    path = Path('workers/tests/fixtures/prompt-render.json.gz')
    rows = json.loads(gzip.decompress(path.read_bytes()))
    changed = 0
    for row in rows:
        if row['kind'] == 'role' and row['document']['role'] in ('ingest', 'audit'):
            result = subprocess.check_output([binary, 'render-prompt', row['kind']],
                                             input=json.dumps(row['document']).encode())
            response = json.loads(result)
            changed += response != row['response']
            row['response'] = response
    path.write_bytes(gzip.compress(json.dumps(rows, ensure_ascii=True, separators=(',', ':')).encode(), mtime=0))
    print('Updated real-Rust ingest/audit prompt recordings:', changed, flush=True)
    run('python3', 'tools/ci/check.py', 'python-lint')
    run('python3', 'tools/bazel/requirements.py', '--check')
    artifact = Path(os.environ['BUILDBUDDY_ARTIFACTS_DIRECTORY'])
    (artifact / 'ep-generated.patch').write_bytes(subprocess.check_output(['git', 'diff', '--binary']))


def identities():
    sys.path.insert(0, str(Path('workers/tests').resolve()))
    import prompt_identity_vectors as vectors
    from tl3_workers import prompt_render
    # Capture the current Python fallback, then qualify against real Rust below.
    token = prompt_render.selection.set({})
    try:
        generated = vectors.generate()
    finally:
        prompt_render.selection.reset(token)
    previous = json.loads(vectors.VECTORS.read_text())
    for role in previous['roles']:
        if role not in ('ingest', 'audit'):
            assert generated['roles'][role] == previous['roles'][role], role
    print('Unchanged prompt identity roles:', ', '.join(sorted(set(previous['roles']) - {'ingest', 'audit'})), flush=True)
    vectors.VECTORS.write_text(json.dumps(generated, indent=2, ensure_ascii=False) + '\n')


if __name__ == '__main__':
    {'prepare': prepare, 'identities': identities}[sys.argv[1]]()
