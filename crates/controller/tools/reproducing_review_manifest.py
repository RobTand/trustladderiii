"""Describe PR 107's fix-round checks. Submit this output with pbcampaign.py."""
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('suite', choices=('hosts', 'python', 'harness'))
args = parser.parse_args()
root = Path(__file__).resolve().parents[3]
env = {
    'PATH': '/home/rob/.cargo/bin:/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin',
    'RUSTUP_HOME': '/home/rob/.rustup', 'CARGO_HOME': '/home/rob/.cargo',
    'KANI_HOME': '/home/rob/.kani', 'CARGO_BUILD_JOBS': '2', 'RUST_TEST_THREADS': '2',
    'OMP_NUM_THREADS': '1', 'MKL_NUM_THREADS': '1', 'OPENBLAS_NUM_THREADS': '1',
    'UV_CONCURRENT_DOWNLOADS': '2', 'UV_CONCURRENT_BUILDS': '2',
    'TL3_TEST_UV': '/home/rob/.local/bin/uv',
}
common = {'cwd': str(root), 'cpus': 2, 'demand': {'mem_gb': 6}, 'env': env, 'priority': -10}
if args.suite == 'hosts':
    scenarios = ['cargo', 'test', '--locked', '-p', 'tl3-controller', '--test', 'workers',
                 '--test', 'prismabuild', '--test', 'publishing', '--test', 'reproducing_rebase']
    rows = [dict(common, tags=[host], argv=(['bash', 'crates/controller/tools/verify.sh']
            if host == 'dl380g10' else scenarios)) for host in ('dl380g10', 'sparky', 'sparklina')]
elif args.suite == 'python':
    command = '''set -e
/home/rob/.local/bin/uv tool run --from ruff==0.14.14 ruff check --isolated --select E9,F crates/controller/tests crates/controller/tools workers/src workers/tests harness/src harness/tools harness/tests
/home/rob/.local/bin/uv run --locked --project workers --python 3.14+gil pytest -c workers/pyproject.toml workers/tests -q
/home/rob/.local/bin/uv run --locked --project workers --python 3.14+gil pytest -c harness/pyproject.toml harness/tests -q'''
    rows = [dict(common, anywhere=True, argv=['bash', '-c', command])]
else:
    # This x86 image is installed on dl380g10; both Sparks lack it.
    image = 'sha256:495578f3075cd49927acfcad6cf8076775e1291de5a9dc7fd1dd7e937b21b525'
    rows = [dict(common, tags=['dl380g10'], cpus=1, demand={'mem_gb': 2},
                 env=dict(env, PYTHONPATH='harness/src'), container_images=[image],
                 progress_phases=['tests=900'],
                 argv=['/usr/bin/python3.14', 'harness/tools/verify.py', image])]
print(json.dumps(rows, indent=2))
