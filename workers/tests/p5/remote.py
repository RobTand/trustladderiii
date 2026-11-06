"""BuildBuddy-only generation and recording steps for the P5 slice."""
import ast
import gzip
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def run(*args, **kwargs):
    return subprocess.run(list(args), check=True, **kwargs)


def bootstrap():
    tree = ast.parse(Path('tools/ci/remote_gate.py').read_text())
    constants = {}
    for node in tree.body:
        if isinstance(node, ast.Assign) and any(isinstance(target, ast.Name) and target.id in
                {'UV_VERSION', 'UV_SHA256', 'INSTALL_UV'} for target in node.targets):
            exec(compile(ast.Module(body=[node], type_ignores=[]), 'uv-bootstrap', 'exec'), constants)
    run('bash', '-c', constants['INSTALL_UV'])
    os.environ['PATH'] = str(Path.home() / '.local/bin') + ':' + os.environ['PATH']
    directory = Path('/home/buildbuddy/p5-tools')
    directory.mkdir(exist_ok=True)
    cargo = directory / 'cargo'
    cargo.write_text('#!/bin/sh\nexec bazel run --config=remote --jobs=16 @rules_rust//tools/upstream_wrapper:cargo -- "$@"\n')
    cargo.chmod(0o755)
    os.environ['CARGO'] = str(cargo)
    return cargo


def artifact(name, content):
    Path(os.environ['BUILDBUDDY_ARTIFACTS_DIRECTORY'], name).write_bytes(content)


def main():
    mode = sys.argv[1]
    scratch = Path('/home/buildbuddy/p5-test-runtime')
    scratch.mkdir(exist_ok=True)
    os.environ['TEST_TMPDIR'] = str(scratch)
    if mode == 'generate':
        bootstrap()
        run('bash', 'tools/bazel/generate.sh')
        run('bazel', 'build', '--config=remote', '--jobs=16', '--remote_download_toplevel', '//crates/controller:tl3')
        binary = Path('bazel-bin/crates/controller/tl3').resolve()
        run(str(binary), 'redaction-policy', '--check', 'crates/controller/src/redaction/policy.json')
        run(str(binary), 'redaction-policy', '--check', 'workers/src/tl3_workers/policy.json')
        run(str(binary), 'redaction-policy', '--identity-check', 'workers/src/tl3_workers/_policy_identity.py')
        schema = subprocess.check_output([str(binary), 'check-quota', 'schema'])
        Path('workers/src/tl3_workers/schemas/quota-event.json').write_bytes(schema)
        artifact('quota-event.json', schema)
        artifact('generated-builds.patch', subprocess.check_output(['git', 'diff', '--', '*/BUILD.bazel', 'tools/bazel/requirements.txt']))
        vectors = subprocess.check_output(['bazel', 'run', '--config=remote', '--jobs=16',
            '//tools/bazel:worker_python', '--', str(Path('workers/tests/p5/quota_vectors.py').resolve())])
        artifact('quota-vectors.json', vectors)
        run('python3', 'tools/ci/check.py', 'python-lint')
        run('python3', 'tools/bazel/requirements.py', '--check')
        return
    if mode == 'graph':
        sys.path.insert(0, str(Path('tools/ci').resolve()))
        from bazel_gate import Workspace
        head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
        base = sys.argv[2]
        workspace = Workspace(root=Path.cwd())
        workspace.ensure_commit(base)
        result = workspace.graph_check(base, head)
        artifact('graph-impact.json', json.dumps(result, indent=2).encode())
        if result['errors']:
            raise RuntimeError(result['errors'])
        return
    if mode == 'mutant':
        source = Path('crates/controller-core/src/worker_events.rs')
        source.write_text(source.read_text().replace('Some(whole + 1)', 'Some(whole)'))
        run('bazel', 'test', '--config=remote', '--jobs=16', '//crates/controller-core:verus_worker_events_quota_reset', '--nocache_test_results')
        raise RuntimeError('ceil mutant escaped its proof')
    if mode == 'features':
        cargo = bootstrap()
        os.environ['PATH'] = str(cargo.parent) + ':' + os.environ['PATH']
        os.environ['CARGO_BUILD_JOBS'] = '4'
        run('python3', 'tools/ci/child_env.py', 'python3', 'tools/ci/feature_gate.py', 'features')
        artifact('feature-builds.json', Path('target/ci-feature-times-features.json').read_bytes())
        return
    if mode == 'record':
        run('bazel', 'build', '--config=remote', '--jobs=16', '--remote_download_toplevel', '//crates/controller:tl3', '@o200k_base//file')
        binary = Path('bazel-bin/crates/controller/tl3').resolve()
        output_base = Path(subprocess.check_output(['bazel', 'info', 'output_base'], text=True).strip())
        tokenizer = list((output_base / 'external').glob('*o200k_base*/file/fb374d419588a4632f3f557e76b4b70aebbca790'))
        assert len(tokenizer) == 1, 'pinned tokenizer input unavailable'
        cache = scratch / 'data-gym-cache'
        cache.mkdir(exist_ok=True)
        import shutil
        shutil.copyfile(tokenizer[0], cache / tokenizer[0].name)
        selected = sys.argv[2:]
        with tempfile.TemporaryDirectory(prefix='p5-record-') as directory:
            environment = {**os.environ, 'TL3_BINARY': str(binary), 'TEST_REAL_CHECKER': '0',
                           'TEST_P5_BINARY': str(binary),
                           'TEST_RECORD_P5_DIR': directory}
            driver = 'import os, pathlib, subprocess, sys\nfrom concurrent.futures import ThreadPoolExecutor\nroot = pathlib.Path(' + repr(str(Path.cwd())) + ')\n' + '''
os.chdir(root / 'workers')
def record(module):
    return subprocess.run([sys.executable, '-m', 'pytest', '-q', module]).returncode
with ThreadPoolExecutor(max_workers=4) as pool:
    results = list(pool.map(record, sys.argv[1:]))
raise SystemExit(max(results))
'''
            selected = [path.removeprefix('workers/') for path in selected]
            result = subprocess.run(['bazel', 'run', '--config=remote', '--jobs=16',
                '//tools/bazel:worker_python', '--', '-c', driver, *selected], env=environment)
            corpus = {}
            for path in sorted(Path(directory).glob('*.json')):
                corpus.update(json.loads(path.read_text()))
            data = json.dumps(corpus, ensure_ascii=False, separators=(',', ':')).encode()
            artifact('p5-checker-vectors.json.gz', gzip.compress(data, mtime=0))
            print(f'P5 recorded {len(corpus)} scenarios, {sum(map(len, corpus.values()))} calls', flush=True)
            raise SystemExit(result.returncode)
    raise RuntimeError('unknown P5 remote phase')


if __name__ == '__main__':
    main()
