"""Regenerate WR3 outputs on BuildBuddy and publish a source-only patch."""
import os
from pathlib import Path
import subprocess


def run(*command):
    subprocess.run(command, check=True)


run('bazel', 'run', '--config=remote', '--jobs=16',
    '//crates/controller:tl3-contracts-schemas', '--', '--root', str(Path.cwd()))
run('bash', 'tools/bazel/generate.sh')
run('python3', 'crates/controller/tools/redaction_policy.py')
artifact = Path(os.environ['BUILDBUDDY_ARTIFACTS_DIRECTORY'], 'wr3-generated.patch')
with artifact.open('w') as output:
    subprocess.run(['git', 'diff', '--', 'workers/src/tl3_workers/schemas',
                    'crates/controller/BUILD.bazel', 'crates/controller/src/redaction/policy.json'],
                   check=True, stdout=output)
print('Generated source patch:', artifact.name)
