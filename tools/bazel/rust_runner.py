"""Run one Rust test binary with the existing gate's budgets and result checks."""
import json
import os
from pathlib import Path
import subprocess
import sys

# Import siblings by path: a crate's own tools/ directory can shadow ours.
sys.path[:0] = [str(Path(__file__).parent), str(Path('tools/ci').resolve())]
from runtime import prepare  # noqa: E402
import libtest_shards  # noqa: E402
import rust_gate  # noqa: E402
from child_env import child_env  # noqa: E402

# Must match WORKSPACE in tools/bazel/rust.bzl.
WORKSPACE = Path('/tmp/tl3-workspace')
SHARD_AWARE_BINARIES = {
    'tl3-controller/github-flow',
    'tl3-controller/github-notices',
    'tl3-controller/mergify',
    'tl3-controller/publishing',
    'tl3-controller/simulation',
    'tl3-controller/workers',
}


def link_workspace():
    """Point the compiled-in CARGO_MANIFEST_DIR root at this test's runfiles."""
    staged = WORKSPACE.with_name(f'{WORKSPACE.name}.{os.getpid()}')
    staged.symlink_to(Path.cwd())
    os.replace(staged, WORKSPACE)


def check_workspace():
    if WORKSPACE.readlink() != Path.cwd():
        raise RuntimeError(f'{WORKSPACE} changed during the test; Rust tests need a private /tmp')


def use_worker_python(launcher):
    """Controller tests run `uv sync` and then workers/.venv/bin/python."""
    venv = Path('workers/.venv/bin')
    venv.mkdir(parents=True, exist_ok=True)
    (venv / 'python').symlink_to(Path(launcher).resolve())
    uv = Path(os.environ['TEST_TMPDIR'], 'uv')
    uv.write_text('#!/bin/sh\nset -eu\n[ "$1" = sync ]\n[ -x workers/.venv/bin/python ]\n')
    uv.chmod(0o755)
    os.environ['TL3_TEST_UV'] = str(uv)


def list_tests(command):
    output = subprocess.run([*command, '--list', '--format=terse'], check=True, capture_output=True,
                            text=True, env=child_env()).stdout
    return libtest_shards.listed(output)


def enforce_budget(results, config, runner, outputs, baseline=None, cpu_seconds=None, cpu_fallbacks=None):
    warnings = rust_gate.check_budget(results, config, baseline or config, runner_class=runner,
                                      cpu_seconds=cpu_seconds, cpu_fallbacks=cpu_fallbacks)
    Path(outputs, 'budget-warnings.json').write_text(json.dumps(warnings) + '\n')
    for warning in warnings:
        print(json.dumps({'budget_warning': warning}), flush=True)


def main():
    prepare()
    link_workspace()
    executable, name, harness, worker_python, *arguments = sys.argv[1:]
    if worker_python != '-':
        use_worker_python(worker_python)
    for repository in Path('..').glob('*postgres_linux_*'):
        os.environ['TL3_TEST_POSTGRES_DIR'] = str(repository.resolve())
    binary = {'name': name, 'command': [str(Path(executable).resolve())]}
    if name.startswith('tl3-controller/'):
        binary['command'] = ['bash', str(Path('tools/ci/controller_test_runner.sh').resolve()),
                             '--controller', *binary['command']]
    shard = None
    if int(os.environ.get('TEST_TOTAL_SHARDS', '1')) > 1:
        shard = {'index': int(os.environ['TEST_SHARD_INDEX']),
                 'total': int(os.environ['TEST_TOTAL_SHARDS'])}
        if harness == 'libtest':
            weights = json.loads(libtest_shards.WEIGHTS.read_text()).get(name, {})
            selected = libtest_shards.select(list_tests(binary['command']), weights,
                                             shard['index'], shard['total'])
            arguments = ['--exact', *selected, *arguments]
        elif name not in SHARD_AWARE_BINARIES:
            raise RuntimeError('scenario sharding requires a shard-aware custom harness')
        Path(os.environ['TEST_SHARD_STATUS_FILE']).touch()
    cpu_seconds, cpu_details = {}, {}
    results = rust_gate.execute_binary(binary, harness == 'custom', arguments, cpu_seconds=cpu_seconds,
                                       cpu_details=cpu_details)
    check_workspace()
    # .bazelrc names the executor class used to calculate warning thresholds.
    runner = os.environ.get('TL3_RUNNER_CLASS', 'baseline')
    names = [key.split('::', 1)[1] for key in results if not key.endswith('::__tail__')]
    # Written before the budget check so calibration also sees overruns.
    receipt = Path(os.environ['TEST_UNDECLARED_OUTPUTS_DIR'], 'inventory.json')
    receipt.write_text(json.dumps({'rust': {name: names}, 'seconds': results, 'cpu_seconds': cpu_seconds,
                                   'runner_class': runner, 'shard': shard, 'cpu_platform': sys.platform,
                                   'cpu_sampling': 'proc-postmaster-v1', **cpu_details}))
    if os.environ.get('TL3_BUDGETS') == 'record':
        # Uncalibrated executors (arm64) keep the seconds in the receipt only.
        return
    config = json.loads(Path('tools/ci/rust-test-budget.json').read_text())
    enforce_budget(results, config, runner, os.environ['TEST_UNDECLARED_OUTPUTS_DIR'],
                   cpu_seconds=cpu_seconds, cpu_fallbacks=cpu_details['cpu_fallbacks'])


if __name__ == '__main__':
    main()
