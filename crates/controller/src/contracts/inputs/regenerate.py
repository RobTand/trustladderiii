"""Regenerate WR3 outputs on BuildBuddy and publish a source-only patch."""
import os
from pathlib import Path
import subprocess
import sys


def run(*command):
    subprocess.run(command, check=True)


os.environ['PATH'] = f'{Path.home()}/.local/bin:{Path.home()}/.cargo/bin:' + os.environ['PATH']
sys.path.insert(0, str(Path('tools/ci').resolve()))
from remote_gate import INSTALL_UV  # noqa: E402

run('bash', '-c', INSTALL_UV)
try:
    run('bazel', 'run', '--config=remote', '--jobs=16',
        '//crates/controller:tl3-contracts-schemas', '--', '--root', str(Path.cwd()))
    output_base = subprocess.check_output(['bazel', 'info', 'output_base'], text=True).strip()
    candidates = sorted(Path(output_base, 'external').glob('*/bin/cargo'))
    if candidates:
        os.environ['CARGO'] = str(candidates[0])
        os.environ['PATH'] = str(candidates[0].parent) + ':' + os.environ['PATH']
    else:
        from kani_gate import INSTALL_KANI
        run('bash', '-c', INSTALL_KANI.split('if [ "$(cargo-kani')[0])
    run('bash', 'tools/bazel/generate.sh')
    python = subprocess.check_output(['uv', 'python', 'find', '3.14'], text=True).strip()
    run(python, 'crates/controller/tools/redaction_policy.py')
finally:
    artifact = Path(os.environ['BUILDBUDDY_ARTIFACTS_DIRECTORY'], 'wr3-generated.patch')
    with artifact.open('w') as output:
        subprocess.run(['git', 'diff', '--', 'workers/src/tl3_workers/schemas',
                        'crates/controller/BUILD.bazel', 'crates/controller/src/redaction/policy.json'],
                       check=True, stdout=output)
    print('Generated source patch:', artifact.name)
