"""Reproduce parametrized check validation through PrismaBuild."""
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('suite', choices=('red', 'python', 'hosts', 'workspace', 'all'))
args = parser.parse_args()
root = Path(__file__).resolve().parents[3]
env = {
    'PATH': '/home/rob/.cargo/bin:/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin',
    'RUSTUP_HOME': '/home/rob/.rustup', 'CARGO_HOME': '/home/rob/.cargo',
    'KANI_HOME': '/home/rob/.kani', 'CARGO_BUILD_JOBS': '2', 'RUST_TEST_THREADS': '2',
    'RAYON_NUM_THREADS': '1', 'OMP_NUM_THREADS': '1', 'MKL_NUM_THREADS': '1',
    'OPENBLAS_NUM_THREADS': '1', 'UV_CONCURRENT_DOWNLOADS': '2',
    'UV_CONCURRENT_BUILDS': '2', 'TL3_TEST_UV': '/home/rob/.local/bin/uv',
    'TMPDIR': '/home/rob/tmp',
}
common = {'cwd': str(root), 'cpus': 2, 'demand': {'mem_gb': 6}, 'env': env, 'priority': -10}
rows = []
if args.suite == 'red':
    rows.append({**common, 'cpus': 1, 'demand': {'mem_gb': 3}, 'anywhere': True,
        'argv': ['/home/rob/.local/bin/uv', 'run', '--locked', '--project', 'workers',
                 '--python', '3.14+gil', 'pytest', '-c', 'workers/pyproject.toml',
                 'harness/tests/test_parametrized_checks.py', 'workers/tests/test_planner.py',
                 'workers/tests/test_maker.py', 'workers/tests/test_tracing.py',
                 '-q', '-k', 'parametrized_bare or existing_named or checks_base_test or new_unique or no_change_done or failed_worker_trace or done_without']})
if args.suite in {'python', 'all'}:
    rows.append({**common, 'tags': ['dl380g10'], 'cpus': 4, 'demand': {'mem_gb': 8},
        'argv': ['bash', '-ec', '''
cargo build --locked -p tl3-controller --example tl3-deploy
export TL3_DEPLOY_HELPER="$PWD/target/debug/examples/tl3-deploy"
/usr/bin/python3.14 tools/ci/check.py python
''']})
if args.suite in {'hosts', 'all'}:
    rows.extend({**common, 'tags': [host], 'argv': ['cargo', 'test', '--locked',
        '-p', 'tl3-controller', '--lib', '--test', 'workers', '--test', 'reproducing_rebase',
        '--test', 'prismabuild']} for host in ('dl380g10', 'sparky'))
if args.suite in {'workspace', 'all'}:
    rows.append({**common, 'anywhere': True,
                 'argv': ['bash', 'crates/controller/tools/verify.sh']})
print(json.dumps(rows, indent=2))
