"""Real generator producer -> real host collector -> Rust collect_worker. Inside PB only.

Driven by crates/controller/tests/run_backend_cli.rs. The native Flux CLI is the
existing fake; the Rust backend CLI, journal, generator and collector are real.
Only host identity (hostname/cgroup) is a deterministic fixture boundary.
"""
import json
import os
from pathlib import Path
import sys

IMAGE = 'sha256:' + 'b' * 64
SECRET = 'ghp_' + 'Z' * 36
CGROUP = '0::/user.slice/user-1000.slice/user@1000.service/flux.slice/flux-jobs.slice/shell-fABC:sys.service\n'


def action(root, source):
    sys.path.insert(0, str(source / 'harness/src'))
    from tl3_harness import generator, runtime

    snapshot = runtime.Evidence.host_snapshot
    runtime.Evidence.host_snapshot = staticmethod(
        lambda: {**snapshot(), 'hostname': 'dl380g10', 'cgroup': CGROUP})
    os.chdir(root / 'generator-work')
    sys.argv = ['generator', IMAGE]
    generator.main()


def checkout(work, mode):
    tree = work / 'checkout'
    (tree / 'tools').mkdir(parents=True)
    (tree / 'tools/gen.py').write_text("print('generate')\n")
    (tree / 'data.txt').write_text('immutable input\n')
    if mode == 'readonly-checkout':
        (tree / 'tools/gen.py').chmod(0o555)
        (tree / 'data.txt').chmod(0o444)
        (tree / 'tools').chmod(0o500)
        tree.chmod(0o500)
    elif mode == 'prep-failure':
        blocked = tree / f'token-{SECRET}.txt'
        blocked.write_text('unreadable\n')
        blocked.chmod(0)
    else:
        raise ValueError(mode)
    return tree


def release(tree):
    for path in [tree, *tree.rglob('*')]:
        path.chmod(0o700 if path.is_dir() else 0o600)


def main():
    root, source, mode = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]
    if mode == 'action':
        return action(root, source)
    settings = json.loads((root / 'action-fixture.json').read_text())
    output = Path(settings['output_dir'])
    os.environ.update(settings['environment'])
    sys.path.insert(0, str(source / 'harness/src'))
    from tl3_harness import runtime

    pus = ','.join(map(str, sorted(os.sched_getaffinity(0))))
    (root / 'all.json').write_text(json.dumps({'version': 1, 'execution': {
        'R_lite': [{'rank': '0', 'children': {'core': '37'}}], 'nodelist': ['dl380g10']},
        'scheduling': {'graph': {'nodes': [
            {'metadata': {'type': 'node', 'rank': 0, 'id': 0, 'name': 'dl380g10'}},
            {'metadata': {'type': 'core', 'rank': 0, 'id': 37, 'properties': {'os-pus': pus}}}]}}}))
    work = root / 'generator-work'
    work.mkdir()
    tree = checkout(work, mode)
    (work / 'generator.json').write_text(json.dumps({
        'command': 'python3 tools/gen.py', 'generated_files': ['generated/*'], 'protected_paths': [],
        'files': [], 'timeout': 30, 'cpus': 1, 'mem_gb': 1}))
    try:
        code, result = runtime.collect_host_payload(
            [sys.executable, '-I', '-B', __file__, str(root), str(source), 'action'],
            timeout=60, operation='collect_worker')
        outcome = {'code': code, 'result': result, 'error_type': None}
    except Exception as error:
        outcome = {'error_type': type(error).__name__, 'diagnostic': getattr(error, 'diagnostic', None)}
    finally:
        release(tree)
    outcome['scratch_absent'] = not Path(settings['scratch']).exists()
    (output / 'generator-driver.json').write_text(json.dumps(outcome))
    print(json.dumps(outcome), flush=True)


if __name__ == '__main__':
    main()
