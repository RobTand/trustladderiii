"""Run one Rust test binary with the existing gate's budgets and result checks."""
import json
import os
from pathlib import Path
import sys

# Import siblings by path: a crate's own tools/ directory can shadow ours.
sys.path[:0] = [str(Path(__file__).parent), str(Path('tools/ci').resolve())]
from runtime import prepare  # noqa: E402
import rust_gate  # noqa: E402

# Must match WORKSPACE in tools/bazel/rust.bzl.
WORKSPACE = Path('/tmp/tl3-workspace')


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


def main():
    prepare()
    link_workspace()
    executable, name, harness, worker_python, *arguments = sys.argv[1:]
    if worker_python != '-':
        use_worker_python(worker_python)
    for repository in Path('..').glob('*postgres_linux_*'):
        os.environ['TL3_TEST_POSTGRES_DIR'] = str(repository.resolve())
    binary = {'name': name, 'command': [str(Path(executable).resolve())]}
    results = rust_gate.execute_binary(binary, harness == 'custom', arguments)
    check_workspace()
    # .bazelrc names the executor class; budgets stay strict without one.
    runner = os.environ.get('TL3_RUNNER_CLASS', 'baseline')
    names = [key.split('::', 1)[1] for key in results if not key.endswith('::__tail__')]
    # Written before the budget check so calibration also sees overruns.
    receipt = Path(os.environ['TEST_UNDECLARED_OUTPUTS_DIR'], 'inventory.json')
    receipt.write_text(json.dumps({'rust': {name: names}, 'seconds': results, 'runner_class': runner}))
    config = json.loads(Path('tools/ci/rust-test-budget.json').read_text())
    rust_gate.check_budget(results, config, config, runner_class=runner)


if __name__ == '__main__':
    main()
